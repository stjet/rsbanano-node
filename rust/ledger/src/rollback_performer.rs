use std::sync::atomic::Ordering;

use rsnano_core::{
    utils::seconds_since_epoch, Account, AccountInfo, Amount, BlockEnum, BlockHash, BlockSubType,
    ConfirmationHeightInfo, Epoch, PendingInfo, PendingKey,
};
use rsnano_store_traits::WriteTransaction;

use super::Ledger;

pub(crate) struct BlockRollbackPerformer<'a> {
    ledger: &'a Ledger,
    pub txn: &'a mut dyn WriteTransaction,
    pub rolled_back: Vec<BlockEnum>,
}

impl<'a> BlockRollbackPerformer<'a> {
    pub(crate) fn new(ledger: &'a Ledger, txn: &'a mut dyn WriteTransaction) -> Self {
        Self {
            ledger,
            txn,
            rolled_back: Vec::new(),
        }
    }

    pub(crate) fn roll_back_block_hash(
        mut self,
        block_hash: &BlockHash,
    ) -> anyhow::Result<Vec<BlockEnum>> {
        let block = self.load_block(block_hash)?;
        while self.block_exists(block_hash) {
            self.ensure_block_is_not_confirmed(&block)?;
            let head_block = self.load_account_head(&block)?;
            self.roll_back_head_block(&head_block)?;
            self.rolled_back.push(head_block.clone());
        }

        Ok(self.rolled_back)
    }

    fn load_account_head(&self, block: &BlockEnum) -> anyhow::Result<BlockEnum> {
        let account_info = self.get_account_info(block);
        self.load_block(&account_info.head)
    }

    fn get_account_info(&self, block: &BlockEnum) -> AccountInfo {
        self.ledger
            .store
            .account()
            .get(self.txn.txn(), &block.account_calculated())
            .unwrap()
    }

    fn ensure_block_is_not_confirmed(&self, block: &BlockEnum) -> anyhow::Result<()> {
        let conf_height = self.account_confirmation_height(block);

        if block.sideband().unwrap().height <= conf_height.height {
            bail!("Only unconfirmed blocks can be rolled back")
        }

        Ok(())
    }

    fn account_confirmation_height(&self, block: &BlockEnum) -> ConfirmationHeightInfo {
        self.ledger
            .store
            .confirmation_height()
            .get(self.txn.txn(), &block.account_calculated())
            .unwrap_or_default()
    }

    fn block_exists(&self, block_hash: &BlockHash) -> bool {
        self.ledger.store.block().exists(self.txn.txn(), block_hash)
    }

    pub(crate) fn roll_back_head_block(&mut self, head_block: &BlockEnum) -> anyhow::Result<()> {
        let account = self.get_account(head_block)?;
        let current_account_info = self.load_account(&account);
        let previous_representative = self.get_representative(&head_block.previous())?;

        let previous = if head_block.previous().is_zero() {
            None
        } else {
            Some(self.load_block(&head_block.previous())?)
        };

        let previous_balance = previous
            .as_ref()
            .map(|b| b.balance_calculated())
            .unwrap_or_default();

        let sub_type = if current_account_info.balance < previous_balance {
            BlockSubType::Send
        } else if current_account_info.balance > previous_balance {
            if head_block.is_open() {
                BlockSubType::Open
            } else {
                BlockSubType::Receive
            }
        } else if self.ledger.is_epoch_link(&head_block.link()) {
            BlockSubType::Epoch
        } else {
            BlockSubType::Change
        };

        match sub_type {
            BlockSubType::Send => {
                let destination = head_block.destination().unwrap_or(head_block.link().into());
                self.roll_back_destination_account_until_send_block_is_unreceived(
                    destination,
                    head_block.hash(),
                )?;

                let pending_key = PendingKey::new(destination, head_block.hash());
                self.ledger.store.pending().del(self.txn, &pending_key);
            }
            BlockSubType::Receive | BlockSubType::Open => {
                let source_hash = head_block.source().unwrap_or(head_block.link().into());
                // Pending account entry can be incorrect if source block was pruned. But it's not affecting correct ledger processing
                let linked_account = self
                    .ledger
                    .account(self.txn.txn(), &source_hash)
                    .unwrap_or_default();

                self.ledger.store.pending().put(
                    self.txn,
                    &PendingKey::new(account, source_hash),
                    &PendingInfo::new(
                        linked_account,
                        current_account_info.balance - previous_balance,
                        head_block.sideband().unwrap().source_epoch,
                    ),
                );
            }
            _ => {}
        }

        let previous_account_info =
            self.previous_account_info(head_block, &current_account_info, previous_representative);

        self.ledger.update_account(
            self.txn,
            &account,
            &current_account_info,
            &previous_account_info,
        );

        self.ledger.store.block().del(self.txn, &head_block.hash());

        if head_block.is_legacy() {
            self.ledger
                .store
                .frontier()
                .del(self.txn, &head_block.hash());
            if let Some(previous) = &previous {
                self.ledger
                    .store
                    .frontier()
                    .put(self.txn, &previous.hash(), &account)
            }
        }

        if let Some(previous) = &previous {
            self.ledger
                .store
                .block()
                .successor_clear(self.txn, &previous.hash());
        }

        self.roll_back_representative_cache(
            &current_account_info.representative,
            &current_account_info.balance,
            previous_representative,
            previous_balance,
        );

        self.ledger.cache.block_count.fetch_sub(1, Ordering::SeqCst);
        self.ledger.observer.block_rolled_back(sub_type);
        Ok(())
    }

    /*************************************************************
     * Helper Functions
     *************************************************************/

    fn get_account(&self, block: &BlockEnum) -> anyhow::Result<Account> {
        self.ledger
            .account(self.txn.txn(), &block.hash())
            .ok_or_else(|| anyhow!("account not found"))
    }

    fn roll_back_destination_account_until_send_block_is_unreceived(
        &mut self,
        destination_account: Account,
        send_block: BlockHash,
    ) -> anyhow::Result<()> {
        let pending_key = PendingKey::new(destination_account, send_block);
        loop {
            if self
                .ledger
                .store
                .pending()
                .get(self.txn.txn(), &pending_key)
                .is_some()
            {
                return Ok(());
            }

            self.recurse_roll_back(&self.latest_block_for_account(&pending_key.account)?)?;
        }
    }

    fn recurse_roll_back(&mut self, block_hash: &BlockHash) -> anyhow::Result<()> {
        let mut rolled_back = self.ledger.rollback(self.txn, block_hash)?;
        self.rolled_back.append(&mut rolled_back);
        Ok(())
    }

    fn latest_block_for_account(&self, account: &Account) -> anyhow::Result<BlockHash> {
        self.ledger
            .latest(self.txn.txn(), account)
            .ok_or_else(|| anyhow!("no latest block found"))
    }

    fn roll_back_receive_in_representative_cache(&self, representative: &Account, amount: Amount) {
        self.ledger
            .cache
            .rep_weights
            .representation_add(*representative, Amount::zero().wrapping_sub(amount));
    }

    fn roll_back_representative_cache(
        &self,
        current_rep: &Account,
        current_balance: &Amount,
        previous_rep: Option<Account>,
        previous_balance: Amount,
    ) {
        if let Some(previous_rep) = previous_rep {
            self.roll_back_change_in_representative_cache(
                current_rep,
                current_balance,
                &previous_rep,
                &previous_balance,
            );
        } else {
            self.roll_back_receive_in_representative_cache(current_rep, *current_balance)
        }
    }

    fn roll_back_change_in_representative_cache(
        &self,
        current_representative: &Account,
        current_balance: &Amount,
        previous_representative: &Account,
        previous_balance: &Amount,
    ) {
        self.ledger.cache.rep_weights.representation_add_dual(
            *current_representative,
            Amount::zero().wrapping_sub(*current_balance),
            *previous_representative,
            *previous_balance,
        );
    }

    fn previous_account_info(
        &self,
        block: &BlockEnum,
        current_info: &AccountInfo,
        previous_rep: Option<Account>,
    ) -> AccountInfo {
        if block.previous().is_zero() {
            Default::default()
        } else {
            AccountInfo {
                head: block.previous(),
                representative: previous_rep.unwrap_or(current_info.representative),
                open_block: current_info.open_block,
                balance: self.ledger.balance(self.txn.txn(), &block.previous()),
                modified: seconds_since_epoch(),
                block_count: current_info.block_count - 1,
                epoch: self.get_block_version(&block.previous()),
            }
        }
    }

    fn load_account(&self, account: &Account) -> AccountInfo {
        self.ledger
            .store
            .account()
            .get(self.txn.txn(), account)
            .unwrap_or_default()
    }

    fn load_block(&self, block_hash: &BlockHash) -> anyhow::Result<BlockEnum> {
        self.ledger
            .store
            .block()
            .get(self.txn.txn(), block_hash)
            .ok_or_else(|| anyhow!("block not found"))
    }

    fn get_representative(&self, block_hash: &BlockHash) -> anyhow::Result<Option<Account>> {
        let rep_block_hash = if !block_hash.is_zero() {
            self.ledger
                .representative_block_hash(self.txn.txn(), block_hash)
        } else {
            BlockHash::zero()
        };

        let previous_rep = if !rep_block_hash.is_zero() {
            let rep_block = self.load_block(&rep_block_hash)?;
            Some(rep_block.representative().unwrap_or_default())
        } else {
            None
        };
        Ok(previous_rep)
    }

    fn get_block_version(&self, block_hash: &BlockHash) -> Epoch {
        self.ledger
            .store
            .block()
            .version(self.txn.txn(), block_hash)
    }
}

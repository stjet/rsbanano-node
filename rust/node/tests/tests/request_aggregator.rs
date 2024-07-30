use std::{sync::Arc, time::Duration};

use super::helpers::{assert_timely, assert_timely_eq, System};
use rsnano_core::{Amount, BlockEnum, BlockHash, KeyPair, StateBlock, DEV_GENESIS_KEY};
use rsnano_ledger::{DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH};
use rsnano_messages::ConfirmAck;
use rsnano_node::{
    config::FrontiersConfirmationMode,
    stats::{DetailType, Direction, StatType},
    transport::ChannelEnum,
    wallets::WalletsExt,
};

#[test]
fn one() {
    let mut system = System::new();
    let mut config = System::default_config();
    config.frontiers_confirmation = FrontiersConfirmationMode::Disabled;
    let node = system.build_node().config(config).finish();
    node.wallets
        .insert_adhoc2(
            &node.wallets.wallet_ids()[0],
            &DEV_GENESIS_KEY.private_key(),
            true,
        )
        .unwrap();

    let mut send1 = BlockEnum::State(StateBlock::new(
        *DEV_GENESIS_ACCOUNT,
        *DEV_GENESIS_HASH,
        *DEV_GENESIS_ACCOUNT,
        Amount::MAX - Amount::nano(1000),
        (*DEV_GENESIS_ACCOUNT).into(),
        &DEV_GENESIS_KEY,
        node.work_generate_dev((*DEV_GENESIS_HASH).into()),
    ));

    let request = vec![(send1.hash(), send1.root())];

    // Not yet in the ledger
    let dummy_channel = Arc::new(ChannelEnum::new_null());
    node.request_aggregator
        .request(request.clone(), dummy_channel.clone());
    assert_timely(
        Duration::from_secs(3),
        || node.request_aggregator.is_empty(),
        "aggregator not empty",
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsUnknown,
                Direction::In,
            )
        },
        1,
    );

    // Process and confirm
    node.ledger
        .process(&mut node.ledger.rw_txn(), &mut send1)
        .unwrap();
    node.confirm(send1.hash());

    // In the ledger but no vote generated yet
    node.request_aggregator
        .request(request.clone(), dummy_channel.clone());
    assert_timely(
        Duration::from_secs(3),
        || node.request_aggregator.is_empty(),
        "aggregator not empty",
    );
    assert_timely(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsGeneratedVotes,
                Direction::In,
            ) > 0
        },
        "no votes generated",
    );

    // Already cached
    // TODO: This is outdated, aggregator should not be using cache
    node.request_aggregator.request(request, dummy_channel);
    assert_timely(
        Duration::from_secs(3),
        || node.request_aggregator.is_empty(),
        "aggregator not empty",
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Aggregator,
                DetailType::AggregatorAccepted,
                Direction::In,
            )
        },
        3,
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Aggregator,
                DetailType::AggregatorDropped,
                Direction::In,
            )
        },
        0,
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsUnknown,
                Direction::In,
            )
        },
        1,
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsGeneratedVotes,
                Direction::In,
            )
        },
        2,
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsCannotVote,
                Direction::In,
            )
        },
        0,
    );
}

#[test]
fn one_update() {
    let mut system = System::new();
    let mut config = System::default_config();
    config.frontiers_confirmation = FrontiersConfirmationMode::Disabled;
    let node = system.build_node().config(config).finish();
    node.wallets
        .insert_adhoc2(
            &node.wallets.wallet_ids()[0],
            &DEV_GENESIS_KEY.private_key(),
            true,
        )
        .unwrap();

    let key1 = KeyPair::new();

    let send1 = BlockEnum::State(StateBlock::new(
        *DEV_GENESIS_ACCOUNT,
        *DEV_GENESIS_HASH,
        *DEV_GENESIS_ACCOUNT,
        Amount::MAX - Amount::nano(1000),
        key1.public_key().into(),
        &DEV_GENESIS_KEY,
        node.work_generate_dev((*DEV_GENESIS_HASH).into()),
    ));
    node.process(send1.clone()).unwrap();
    node.confirm(send1.hash());

    let send2 = BlockEnum::State(StateBlock::new(
        *DEV_GENESIS_ACCOUNT,
        send1.hash(),
        *DEV_GENESIS_ACCOUNT,
        Amount::MAX - Amount::nano(2000),
        (*DEV_GENESIS_ACCOUNT).into(),
        &DEV_GENESIS_KEY,
        node.work_generate_dev(send1.hash().into()),
    ));
    node.process(send2.clone()).unwrap();
    node.confirm(send2.hash());

    let receive1 = BlockEnum::State(StateBlock::new(
        key1.public_key(),
        BlockHash::zero(),
        *DEV_GENESIS_ACCOUNT,
        Amount::nano(1000),
        send1.hash().into(),
        &key1,
        node.work_generate_dev(key1.public_key().into()),
    ));
    node.process(receive1.clone()).unwrap();
    node.confirm(receive1.hash());

    let dummy_channel = Arc::new(ChannelEnum::new_null());

    let request1 = vec![(send2.hash(), send2.root())];
    node.request_aggregator
        .request(request1, dummy_channel.clone());

    // Update the pool of requests with another hash
    let request2 = vec![(receive1.hash(), receive1.root())];
    node.request_aggregator
        .request(request2, dummy_channel.clone());

    // In the ledger but no vote generated yet
    assert_timely(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsGeneratedVotes,
                Direction::In,
            ) > 0
        },
        "generated votes",
    );
    assert_timely(
        Duration::from_secs(3),
        || node.request_aggregator.is_empty(),
        "aggregator empty",
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Aggregator,
                DetailType::AggregatorAccepted,
                Direction::In,
            )
        },
        2,
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsGeneratedHashes,
                Direction::In,
            )
        },
        2,
    );
    assert_eq!(
        node.stats.count(
            StatType::Aggregator,
            DetailType::AggregatorDropped,
            Direction::In,
        ),
        0
    );
    assert_eq!(
        node.stats.count(
            StatType::Requests,
            DetailType::RequestsUnknown,
            Direction::In,
        ),
        0
    );
    assert_eq!(
        node.stats.count(
            StatType::Requests,
            DetailType::RequestsCachedHashes,
            Direction::In,
        ),
        0
    );
    assert_eq!(
        node.stats.count(
            StatType::Requests,
            DetailType::RequestsCachedVotes,
            Direction::In,
        ),
        0
    );
    assert_eq!(
        node.stats.count(
            StatType::Requests,
            DetailType::RequestsCannotVote,
            Direction::In,
        ),
        0
    );
}

#[test]
fn two() {
    let mut system = System::new();
    let mut config = System::default_config();
    config.frontiers_confirmation = FrontiersConfirmationMode::Disabled;
    let node = system.build_node().config(config).finish();
    node.wallets
        .insert_adhoc2(
            &node.wallets.wallet_ids()[0],
            &DEV_GENESIS_KEY.private_key(),
            true,
        )
        .unwrap();

    let key1 = KeyPair::new();

    let send1 = BlockEnum::State(StateBlock::new(
        *DEV_GENESIS_ACCOUNT,
        *DEV_GENESIS_HASH,
        *DEV_GENESIS_ACCOUNT,
        Amount::MAX - Amount::raw(1),
        key1.public_key().into(),
        &DEV_GENESIS_KEY,
        node.work_generate_dev((*DEV_GENESIS_HASH).into()),
    ));
    node.process(send1.clone()).unwrap();
    node.confirm(send1.hash());

    let send2 = BlockEnum::State(StateBlock::new(
        *DEV_GENESIS_ACCOUNT,
        send1.hash(),
        *DEV_GENESIS_ACCOUNT,
        Amount::MAX - Amount::raw(2),
        (*DEV_GENESIS_ACCOUNT).into(),
        &DEV_GENESIS_KEY,
        node.work_generate_dev(send1.hash().into()),
    ));
    node.process(send2.clone()).unwrap();
    node.confirm(send2.hash());

    let receive1 = BlockEnum::State(StateBlock::new(
        key1.public_key(),
        BlockHash::zero(),
        *DEV_GENESIS_ACCOUNT,
        Amount::raw(1),
        send1.hash().into(),
        &key1,
        node.work_generate_dev(key1.public_key().into()),
    ));
    node.process(receive1.clone()).unwrap();
    node.confirm(receive1.hash());

    let request = vec![
        (send2.hash(), send2.root()),
        (receive1.hash(), receive1.root()),
    ];
    let dummy_channel = Arc::new(ChannelEnum::new_null());

    // Process both blocks
    node.request_aggregator
        .request(request.clone(), dummy_channel.clone());
    // One vote should be generated for both blocks
    assert_timely(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsGeneratedVotes,
                Direction::In,
            ) > 0
        },
        "generated votes",
    );
    assert_timely(
        Duration::from_secs(3),
        || node.request_aggregator.is_empty(),
        "aggregator empty",
    );
    // The same request should now send the cached vote
    node.request_aggregator
        .request(request.clone(), dummy_channel.clone());
    assert_timely(
        Duration::from_secs(3),
        || node.request_aggregator.is_empty(),
        "aggregator empty",
    );
    assert_eq!(
        node.stats.count(
            StatType::Aggregator,
            DetailType::AggregatorAccepted,
            Direction::In,
        ),
        2
    );
    assert_eq!(
        node.stats.count(
            StatType::Aggregator,
            DetailType::AggregatorDropped,
            Direction::In,
        ),
        0
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsUnknown,
                Direction::In,
            )
        },
        0,
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsGeneratedHashes,
                Direction::In,
            )
        },
        4,
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsGeneratedVotes,
                Direction::In,
            )
        },
        2,
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsCannotVote,
                Direction::In,
            )
        },
        0,
    );
    // Make sure the cached vote is for both hashes
    let vote1 = node.history.votes(&send2.root(), &send2.hash(), false);
    let vote2 = node
        .history
        .votes(&receive1.root(), &receive1.hash(), false);
    assert_eq!(vote1.len(), 1);
    assert_eq!(vote2.len(), 1);
    assert!(Arc::ptr_eq(&vote1[0], &vote2[0]));
}

#[test]
fn split() {
    const MAX_VBH: usize = ConfirmAck::HASHES_MAX;
    let mut system = System::new();
    let mut config = System::default_config();
    config.frontiers_confirmation = FrontiersConfirmationMode::Disabled;
    let node = system.build_node().config(config).finish();
    node.wallets
        .insert_adhoc2(
            &node.wallets.wallet_ids()[0],
            &DEV_GENESIS_KEY.private_key(),
            true,
        )
        .unwrap();

    let mut request = Vec::new();
    let mut blocks = Vec::new();
    let mut previous = *DEV_GENESIS_HASH;

    for i in 0..=MAX_VBH {
        let block = BlockEnum::State(StateBlock::new(
            *DEV_GENESIS_ACCOUNT,
            previous,
            *DEV_GENESIS_ACCOUNT,
            Amount::MAX - Amount::raw(i as u128 + 1),
            (*DEV_GENESIS_ACCOUNT).into(),
            &DEV_GENESIS_KEY,
            node.work_generate_dev(previous.into()),
        ));
        previous = block.hash();
        node.process(block.clone()).unwrap();
        request.push((block.hash(), block.root()));
        blocks.push(block);
    }
    // Confirm all blocks
    node.confirm(blocks.last().unwrap().hash());
    assert_eq!(node.ledger.cemented_count(), MAX_VBH as u64 + 2);
    assert_eq!(MAX_VBH + 1, request.len());
    let dummy_channel = Arc::new(ChannelEnum::new_null());
    node.request_aggregator.request(request, dummy_channel);
    // In the ledger but no vote generated yet
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsGeneratedVotes,
                Direction::In,
            )
        },
        2,
    );
    assert!(node.request_aggregator.is_empty());
    // Two votes were sent, the first one for 12 hashes and the second one for 1 hash
    assert_eq!(
        node.stats.count(
            StatType::Aggregator,
            DetailType::AggregatorAccepted,
            Direction::In,
        ),
        1
    );
    assert_eq!(
        node.stats.count(
            StatType::Aggregator,
            DetailType::AggregatorDropped,
            Direction::In,
        ),
        0
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsGeneratedHashes,
                Direction::In,
            )
        },
        255 + 1,
    );
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node.stats.count(
                StatType::Requests,
                DetailType::RequestsGeneratedVotes,
                Direction::In,
            )
        },
        2,
    );
}

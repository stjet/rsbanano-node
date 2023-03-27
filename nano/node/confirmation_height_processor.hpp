#pragma once

#include "nano/lib/blocks.hpp"
#include "nano/lib/rsnano.hpp"

#include <nano/lib/numbers.hpp>
#include <nano/lib/rsnanoutils.hpp>
#include <nano/lib/timer.hpp>
#include <nano/node/confirmation_height_bounded.hpp>
#include <nano/node/confirmation_height_unbounded.hpp>
#include <nano/secure/common.hpp>
#include <nano/secure/store.hpp>

#include <condition_variable>
#include <thread>
#include <unordered_set>

namespace boost
{
class latch;
}
namespace nano
{
class ledger;
class logger_mt;
class write_database_queue;

class confirmation_height_processor final
{
public:
	confirmation_height_processor (nano::ledger &, nano::stats & stats_a, nano::write_database_queue &, std::chrono::milliseconds, nano::logging const &, std::shared_ptr<nano::logger_mt> &, boost::latch & initialized_latch, confirmation_height_mode = confirmation_height_mode::automatic);
	~confirmation_height_processor ();

	void pause ();
	void unpause ();
	void stop ();
	void add (std::shared_ptr<nano::block> const &);
	void run (confirmation_height_mode);
	std::size_t awaiting_processing_size () const;
	bool is_processing_added_block (nano::block_hash const & hash_a) const;
	bool is_processing_block (nano::block_hash const &) const;
	nano::block_hash current () const;

	/*
	 * Called for each newly cemented block
	 * Called from confirmation height processor thread
	 */
	void set_cemented_observer (std::function<void (std::shared_ptr<nano::block> const &)> const &);
	void clear_cemented_observer ();
	/*
	 * Called when the block was added to the confirmation height processor but is already confirmed
	 * Called from confirmation height processor thread
	 */
	void set_block_already_cemented_observer (std::function<void (nano::block_hash const &)> const &);
	size_t unbounded_pending_writes_size () const;

public:
	rsnano::ConfirmationHeightProcessorHandle * handle;

private:
	std::thread thread;

private: // Tests
	friend class confirmation_height_pending_observer_callbacks_Test;
	friend class confirmation_height_dependent_election_Test;
	friend class confirmation_height_dependent_election_after_already_cemented_Test;
	friend class confirmation_height_dynamic_algorithm_no_transition_while_pending_Test;
	friend class confirmation_height_many_accounts_many_confirmations_Test;
	friend class confirmation_height_long_chains_Test;
	friend class confirmation_height_many_accounts_single_confirmation_Test;
	friend class request_aggregator_cannot_vote_Test;
	friend class active_transactions_pessimistic_elections_Test;
};

std::unique_ptr<container_info_component> collect_bounded_container_info (confirmation_height_processor &, std::string const &);
std::unique_ptr<nano::container_info_component> collect_unbounded_container_info (confirmation_height_processor &, std::string const & name_a);
std::unique_ptr<nano::container_info_component> collect_container_info (confirmation_height_processor &, std::string const & name_a);
}

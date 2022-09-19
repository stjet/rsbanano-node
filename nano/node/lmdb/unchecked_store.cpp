#include <nano/node/lmdb/lmdb.hpp>
#include <nano/node/lmdb/unchecked_store.hpp>
#include <nano/secure/parallel_traversal.hpp>

nano::lmdb::unchecked_store::unchecked_store (nano::lmdb::store & store_a) :
	store (store_a),
	handle{ rsnano::rsn_lmdb_unchecked_store_create (store_a.env ().handle) } {};

nano::lmdb::unchecked_store::~unchecked_store ()
{
	rsnano::rsn_lmdb_unchecked_store_destroy (handle);
}

void nano::lmdb::unchecked_store::clear (nano::write_transaction const & transaction_a)
{
	rsnano::rsn_lmdb_unchecked_store_clear (handle, transaction_a.get_rust_handle ());
}

void nano::lmdb::unchecked_store::put (nano::write_transaction const & transaction_a, nano::hash_or_account const & dependency, nano::unchecked_info const & info)
{
	rsnano::rsn_lmdb_unchecked_store_put (handle, transaction_a.get_rust_handle (), dependency.bytes.data (), info.handle);
}

bool nano::lmdb::unchecked_store::exists (nano::transaction const & transaction_a, nano::unchecked_key const & key)
{
	nano::mdb_val value;
	auto status = store.get (transaction_a, tables::unchecked, key, value);
	release_assert (store.success (status) || store.not_found (status));
	return store.success (status);
}

void nano::lmdb::unchecked_store::del (nano::write_transaction const & transaction_a, nano::unchecked_key const & key_a)
{
	auto status (store.del (transaction_a, tables::unchecked, key_a));
	store.release_assert_success (status);
}

nano::store_iterator<nano::unchecked_key, nano::unchecked_info> nano::lmdb::unchecked_store::end () const
{
	return nano::store_iterator<nano::unchecked_key, nano::unchecked_info> (nullptr);
}

nano::store_iterator<nano::unchecked_key, nano::unchecked_info> nano::lmdb::unchecked_store::begin (nano::transaction const & transaction) const
{
	return store.make_iterator<nano::unchecked_key, nano::unchecked_info> (transaction, tables::unchecked);
}

nano::store_iterator<nano::unchecked_key, nano::unchecked_info> nano::lmdb::unchecked_store::lower_bound (nano::transaction const & transaction, nano::unchecked_key const & key) const
{
	return store.make_iterator<nano::unchecked_key, nano::unchecked_info> (transaction, tables::unchecked, key);
}

size_t nano::lmdb::unchecked_store::count (nano::transaction const & transaction_a)
{
	return store.count (transaction_a, tables::unchecked);
}

MDB_dbi nano::lmdb::unchecked_store::table_handle () const
{
	return rsnano::rsn_lmdb_unchecked_store_table_handle (handle);
}

void nano::lmdb::unchecked_store::set_table_handle (MDB_dbi dbi)
{
	rsnano::rsn_lmdb_unchecked_store_set_table_handle (handle, dbi);
}

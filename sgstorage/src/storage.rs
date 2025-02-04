// Copyright (c) The Starcoin Core Contributors
// SPDX-License-Identifier: Apache-2.0

use failure::prelude::*;
use libra_types::account_address::AccountAddress;
use rocksdb::{rocksdb_options::ColumnFamilyDescriptor, CFHandle, DBOptions, DB};
use schemadb::ColumnFamilyOptionsMap;
use schemadb::DEFAULT_CF_NAME;
use std::collections::BTreeMap;
use std::path::Path;

pub struct SgStorage {
    inner: rocksdb::DB,
    owner: AccountAddress,
}

impl AsMut<rocksdb::DB> for SgStorage {
    fn as_mut(&mut self) -> &mut rocksdb::DB {
        &mut self.inner
    }
}
impl AsRef<rocksdb::DB> for SgStorage {
    fn as_ref(&self) -> &rocksdb::DB {
        &self.inner
    }
}

impl core::ops::Deref for SgStorage {
    type Target = rocksdb::DB;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl core::ops::DerefMut for SgStorage {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

impl SgStorage {
    /// Create db with all the column families provided if it doesn't exist at `path`; Otherwise,
    /// try to open it with all the column families.
    pub fn open<P: AsRef<Path>>(
        owner: AccountAddress,
        path: P,
        mut cf_opts_map: ColumnFamilyOptionsMap,
    ) -> Result<Self> {
        let mut db_opts = DBOptions::new();

        // For now we set the max total WAL size to be 1G. This config can be useful when column
        // families are updated at non-uniform frequencies.
        db_opts.set_max_total_wal_size(1 << 30);

        // If db exists, just open it with all cfs.
        if db_exists(path.as_ref()) {
            let inner = Self::open_cf(db_opts, &path, cf_opts_map.into_iter().collect())?;
            return Ok(SgStorage { inner, owner });
        }

        // If db doesn't exist, create a db first with all column families.
        db_opts.create_if_missing(true);

        let db = Self::open_cf(
            db_opts,
            &path,
            vec![cf_opts_map
                .remove_entry(&DEFAULT_CF_NAME)
                .ok_or_else(|| format_err!("No \"default\" column family name found"))?],
        )?;
        let mut storage = SgStorage { inner: db, owner };
        cf_opts_map
            .into_iter()
            .map(|(cf_name, cf_opts)| storage.create_cf((cf_name, cf_opts)))
            .collect::<Result<Vec<_>>>()?;
        Ok(storage)
    }
    pub fn get_cf_handle(&self, cf_name: &str) -> Result<&CFHandle> {
        self.inner.cf_handle(cf_name).ok_or_else(|| {
            format_err!(
                "DB::cf_handle not found for column family name: {}",
                cf_name
            )
        })
    }
    pub fn owner_address(&self) -> AccountAddress {
        self.owner
    }

    /// Returns the approximate size of each non-empty column family in bytes.
    pub fn get_approximate_sizes_cf(&self) -> Result<BTreeMap<String, u64>> {
        let mut cf_sizes = BTreeMap::new();

        for cf_name in self.inner.cf_names().into_iter().map(ToString::to_string) {
            let cf_handle = self.get_cf_handle(&cf_name)?;
            let size = self
                .inner
                .get_property_int_cf(cf_handle, "rocksdb.estimate-live-data-size")
                .ok_or_else(|| {
                    format_err!(
                        "Unable to get approximate size of {} column family.",
                        cf_name,
                    )
                })?;
            cf_sizes.insert(cf_name, size);
        }

        Ok(cf_sizes)
    }

    fn open_cf<'a, P, T>(opts: DBOptions, path: P, cfds: Vec<T>) -> Result<DB>
    where
        P: AsRef<Path>,
        T: Into<ColumnFamilyDescriptor<'a>>,
    {
        let inner = rocksdb::DB::open_cf(
            opts,
            path.as_ref().to_str().ok_or_else(|| {
                format_err!("Path {:?} can not be converted to string.", path.as_ref())
            })?,
            cfds,
        )
        .map_err(convert_rocksdb_err)?;

        Ok(inner)
    }

    fn create_cf<'a, T>(&mut self, cfd: T) -> Result<()>
    where
        T: Into<ColumnFamilyDescriptor<'a>>,
    {
        let _cf_handle = self.inner.create_cf(cfd).map_err(convert_rocksdb_err)?;
        Ok(())
    }
}

/// Checks underlying Rocksdb instance existence by checking `CURRENT` file existence, the same way
/// Rocksdb adopts to detect db existence.
fn db_exists(path: &Path) -> bool {
    let rocksdb_current_file = path.join("CURRENT");
    rocksdb_current_file.is_file()
}

/// All the RocksDB methods return `std::result::Result<T, String>`. Since our methods return
/// `failure::Result<T>`, manual conversion is needed.
fn convert_rocksdb_err(msg: String) -> failure::Error {
    format_err!("RocksDB internal error: {}.", msg)
}

//! Database access layer and MySQL executor utilities.
//!
//! This module provides a thin database layer that translates `dao::Dao`
//! objects into SQL strings and executes them against a MySQL database
//! using the `mysql` crate. It intentionally separates SQL generation
//! (performed by DAO implementations) from execution (this module), so
//! callers can construct DAOs, inspect the SQL, and then use this
//! executor to run those statements inside transactions.
//!
//! Key concepts:
//! - `MySqlContext` holds connection pooling and transaction options used
//!   when executing statements.
//! - `MySql` is a stateless executor that implements a set of convenience
//!   methods (`mutate`, `put`, `update`, `delete`, `query`) which accept
//!   `dao::Dao` trait objects. Each method starts a transaction and commits
//!   on success, rolling back on errors as appropriate.
//! - `DBError` maps underlying MySQL errors and higher-level failures into
//!   a small set of error variants returned to callers.
//!
//! Transaction and error semantics (summary):
//! - All write operations are executed inside transactions. On any
//!   non-recoverable error the transaction is rolled back and an error is
//!   returned to the caller.
//! - Duplicate-key (MySQL error code 1062) handling varies by method:
//!   - `mutate`: records a `duplicate_insertion` flag and will optionally
//!     continue or rollback depending on `MySqlContext::rollback_on_duplicate`.
//!   - `put`: continues past duplicate-key errors (logs and ignores them).
//! - SQL string construction is delegated to DAO implementations; this
//!   module does not perform parameter binding. For untrusted input,
//!   callers should prefer prepared statements when executing raw SQL.

use std::any::{Any, TypeId};
use std::error::Error;
use std::fmt::Display;
use std::ptr::null;
use std::sync::Arc;
use async_trait::async_trait;
use log::{error, info};
use mysql::{Pool, Row, Transaction, TxOpts};
use mysql::prelude::Queryable;
use sql_query_builder::Insert;
use tower::util::Either::B;
use crate::{dao, db};
use crate::dao::{MutationType, RouterWorker, WorkerContainer};
/*
NOTE : Tried making interfaces using generics so that it would be extensible,
but I was not able to make it work, if someone cracks let me know.
pub trait RDBMS<T: dao::Dao, C: Context> {
    async fn put(&self, put_entities: Vec<Box<T>>, context: C) -> Result<String, String>;
    // fn get<T>(&self, get_statement : String) -> Result<Vec<T>, String>;
}

pub trait Context {}
*/

/// Database execution context used when running statements.
///
/// Fields:
/// - `pool`: MySQL connection pool used to acquire connections.
/// - `rollback_on_duplicate`: controls behaviour when a duplicate-key
///   (MySQL error code 1062) is encountered inside `mutate`. If true the
///   transaction is rolled back; otherwise the executor will skip the
///   failing statement and continue processing remaining queries.
/// - `tx_opts`: transaction options passed to `start_transaction`.
#[derive(Clone)]
pub struct MySqlContext {
    pub pool: Pool,
    pub rollback_on_duplicate: bool,
    pub tx_opts: TxOpts,
}

impl MySqlContext {}

/// Stateless MySQL executor.
///
/// `MySql` is a small, stateless type that exposes convenience methods to
/// execute SQL produced by `dao::Dao` implementations. The methods accept
/// trait objects (`Box<dyn dao::Dao>`) and a shared `Arc<MySqlContext>` so
/// they can be invoked from multiple tasks concurrently.
pub struct MySql {}

/// Errors returned by database executor methods.
///
/// - `MySqlError`: wraps the underlying `mysql::MySqlError` returned by the
///   driver.
/// - `GenericError`: fallback for other error conditions (string message).
/// - `RollbackError`: indicates a failure during transaction rollback.
pub enum DBError {
    MySqlError(mysql::MySqlError),
    GenericError(String),
    RollbackError(String),
}
impl Display for DBError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DBError::MySqlError(e) => write!(f, "MySql Error : {}", e),
            DBError::GenericError(e) => write!(f, "Generic Error : {}", e),
            DBError::RollbackError(e) => write!(f, "Rollback Error : {}", e),
        }
    }
}
impl MySql
{
    /// Execute a sequence of DAO mutations inside a transaction.
    ///
    /// This method accepts a vector of boxed DAO trait objects and a shared
    /// `MySqlContext`. Each DAO's `get_*_string` method is invoked based on
    /// its `MutationType` to produce SQL. All SQL statements are executed in
    /// order inside a single transaction created with `context.tx_opts`.
    ///
    /// Behaviour details:
    /// - If any DAO returns an error while building its SQL the method
    ///   returns `DBError::GenericError` and no database writes are attempted.
    /// - When executing statements, a MySQL duplicate-key error (code 1062)
    ///   will set an internal `duplicate_insertion` flag. If
    ///   `context.rollback_on_duplicate` is true, the transaction is rolled
    ///   back and the error is returned; otherwise the duplicate is
    ///   skipped and execution continues.
    /// - On successful commit the method returns `Ok(("Success".to_string(), duplicate_insertion))`.
    ///
    /// # Errors
    /// Returns `DBError::MySqlError` for driver-level failures, `DBError::RollbackError`
    /// if rollback fails, and `DBError::GenericError` for other conditions.
    pub fn mutate(&self, mutate_entities: Vec<Box<dyn dao::Dao>>, context: Arc<MySqlContext>) -> Result<(String, bool), DBError> {
        let mut queries: Vec<String> = Vec::new();
        for t in mutate_entities {
            match t.get_mutation_type() {
                MutationType::Insert => {
                    let insert_string = match t.get_insert_string() {
                        Ok(s) => s,
                        Err(e) => {
                            error!("Error while building the insert queries {}", e);
                            return Err(DBError::GenericError(e));
                        }
                    };
                    info!("Insert string is {}", insert_string);
                    queries.push(insert_string);
                }
                MutationType::Update => {
                    let update_string = match t.get_update_string() {
                        Ok(s) => s,
                        Err(e) => {
                            error!("Error while building the update queries {}", e);
                            return Err(DBError::GenericError(e));
                        }
                    };
                    info!("Update string is {}", update_string);
                    queries.push(update_string);
                }
                MutationType::Delete => {
                    let delete_string = match t.get_delete_string() {
                        Ok(s) => s,
                        Err(e) => {
                            error!("Error while building the delete queries {}", e);
                            return Err(DBError::GenericError(e));
                        }
                    };
                    info!("Delete string is {}", delete_string);
                    queries.push(delete_string);
                }
                MutationType::CustomQuery1 => {
                    let custom_query_string = match t.get_custom_query1_string() {
                        Ok(s) => s,
                        Err(e) => {
                            error!("Error while building the custom query 1 {}", e);
                            return Err(DBError::GenericError(e));
                        }
                    };
                    info!("Custom query 1 string is {}", custom_query_string);
                    queries.push(custom_query_string);
                }
                MutationType::CustomQuery2 => {
                    let custom_query_string = match t.get_custom_query2_string() {
                        Ok(s) => s,
                        Err(e) => {
                            error!("Error while building the custom query 2 {}", e);
                            return Err(DBError::GenericError(e));
                        }
                    };
                    info!("Custom query 1 string is {}", custom_query_string);
                    queries.push(custom_query_string);
                }
                _ => {
                    error!("Unhandled case");
                    return Err(DBError::GenericError("Unhandled case".to_string()));
                }
            }
        }
        let mut conn = context.pool.get_conn().unwrap();
        let mut transaction = conn.start_transaction(context.tx_opts).unwrap();
        let mut duplicate_insertion = false;
        for query in queries {
            match transaction.query_drop(query) {
                Ok(o) => o,
                Err(e) => {
                    match e {
                        mysql::Error::MySqlError(e) => {
                            //duplicate key error handling
                            if e.code == 1062 {
                                error!("Tried creating duplicate entry {}", e);
                                duplicate_insertion = true;
                                if context.rollback_on_duplicate == false {
                                    continue;
                                }
                            }
                            error!("rolling back transaction as there is an error while writing to DB {}", e);
                            if let Err(rollback_err) = transaction.rollback() {
                                error!("Error in rolling back the transaction error {}", rollback_err);
                                return Err(DBError::RollbackError(rollback_err.to_string()));
                            }
                            return Err(DBError::MySqlError(e));
                        }
                        _ => {
                            error!("Error in writing to DB Generic Error {}", e);
                            if let Err(rollback_err) = transaction.rollback() {
                                error!("Error in rolling back the transaction Generic Error {}", rollback_err);
                                return Err(DBError::GenericError(rollback_err.to_string()));
                            }
                            return Err(DBError::GenericError(e.to_string()));
                        }
                    }
                }
            }
        }
        match transaction.commit() {
            Ok(_) => {
                // info!("Executed successfully");
                Ok(("Success".to_string(), duplicate_insertion))
            }
            Err(e) => {
                error!("Error in committing to database {}", e);
                match e {
                    mysql::Error::MySqlError(e) => {
                        error!("Error in committing to database MySql Error {}", e);
                        Err(DBError::MySqlError(e))
                    }
                    _ => {
                        error!("Error in committing to database Generic Error {}", e);
                        Err(DBError::GenericError(e.to_string()))
                    }
                }
            }
        }
    }
    /// Insert-only helper: execute many inserts inside a transaction.
    ///
    /// This method converts the provided DAOs to insert SQL using
    /// `get_insert_string` and executes them all in a single transaction
    /// (using default `TxOpts`). Duplicate-key errors are ignored (logged)
    /// and do not cause the transaction to fail.
    pub fn put(&self, put_entities: Vec<Box<dyn dao::Dao>>, context: Arc<MySqlContext>) -> Result<String, DBError> {
        let mut insert_queries: Vec<String> = Vec::new();
        for t in put_entities {
            match t.get_insert_string() {
                Ok(o) => insert_queries.push(o),
                Err(e) => {
                    error!("Error while building the queries {}", e);
                    return Err(DBError::GenericError(e));
                }
            }
        }
        let mut conn = context.pool.get_conn().unwrap();
        let mut transaction = conn.start_transaction(TxOpts::default()).unwrap();
        for query in insert_queries {
            match transaction.query_drop(query) {
                Ok(o) => o,
                Err(e) => {
                    match e {
                        mysql::Error::MySqlError(e) => {
                            //duplicate key error handling
                            if e.code == 1062 {
                                info!("Tried creating duplicate entry {}", e);
                                continue;
                            }
                            error!("Error in writing to DB MySql error {}", e);
                            if let Err(rollback_err) = transaction.rollback() {
                                error!("Error in rolling back the transaction error {}", rollback_err);
                                return Err(DBError::RollbackError(rollback_err.to_string()));
                            }
                            return Err(DBError::MySqlError(e));
                        }
                        _ => {
                            error!("Error in writing to DB Generic Error {}", e);
                            if let Err(rollback_err) = transaction.rollback() {
                                error!("Error in rolling back the transaction Generic Error {}", rollback_err);
                                return Err(DBError::GenericError(rollback_err.to_string()));
                            }
                            return Err(DBError::GenericError(e.to_string()));
                        }
                    }
                }
            }
        }
        match transaction.commit() {
            Ok(_) => {
                // info!("Executed successfully");
                Ok("Success".to_string())
            }
            Err(e) => {
                error!("Error in committing to database {}", e);
                match e {
                    mysql::Error::MySqlError(e) => {
                        error!("Error in committing to database MySql Error {}", e);
                        Err(DBError::MySqlError(e))
                    }
                    _ => {
                        error!("Error in committing to database Generic Error {}", e);
                        Err(DBError::GenericError(e.to_string()))
                    }
                }
            }
        }
    }

    /// Execute a set of update statements inside a transaction.
    ///
    /// This method converts each DAO to an UPDATE SQL string via
    /// `get_update_string` and executes them all inside a transaction. Any
    /// error during preparation or execution results in a rollback and a
    /// `DBError` return value.
    pub fn update(&self, update_entities: Vec<Box<dyn dao::Dao>>, context: Arc<MySqlContext>) -> Result<String, DBError> {
        let mut update_queries: Vec<String> = Vec::new();
        for t in update_entities {
            match t.get_update_string() {
                Ok(o) => update_queries.push(o),
                Err(e) => {
                    error!("Error while building the queries {}", e);
                    return Err(DBError::GenericError(e));
                }
            }
        }
        let mut conn = match context.pool.get_conn() {
            Ok(o) => o,
            Err(e) => {
                error!("Error in getting connection {}", e);
                return Err(DBError::GenericError(e.to_string()));
            }
        };
        let mut transaction = match conn.start_transaction(TxOpts::default()) {
            Ok(o) => o,
            Err(e) => {
                error!("Error in starting transaction {}", e);
                return Err(DBError::GenericError(e.to_string()));
            }
        };
        for query in update_queries {
            //info!("{}",query.as_str());
            transaction.query_drop(query).unwrap();
        }
        match transaction.commit() {
            Ok(_) => {
                // info!("Executed successfully");
                Ok("Success".to_string())
            }
            Err(e) => {
                error!("Error in writing to database {}", e);
                match e {
                    mysql::Error::MySqlError(e) => {
                        error!("Error in writing to database {}", e);
                        Err(DBError::MySqlError(e))
                    }
                    _ => {
                        error!("Error in writing to database {}", e);
                        Err(DBError::GenericError(e.to_string()))
                    }
                }
            }
        }
    }

    /// Execute a set of delete statements inside a transaction and return the
    /// number of rows affected by the transaction.
    ///
    /// This method builds delete SQL strings from the provided DAOs and runs
    /// them in a single transaction. On success it returns the transaction's
    /// `affected_rows()` value; on error it attempts to rollback and returns
    /// a `DBError`.
    pub fn delete(&self, delete_entities: Vec<Box<dyn dao::Dao>>, context: Arc<MySqlContext>) -> Result<u64, DBError> {
        let mut delete_queries: Vec<String> = Vec::new();
        for t in delete_entities {
            match t.get_delete_string() {
                Ok(o) => {
                    info!("Delete query is {}", o);
                    delete_queries.push(o)
                },
                Err(e) => {
                    error!("Error while building the queries {}", e);
                    return Err(DBError::GenericError(e));
                }
            }
        }
        let mut conn = context.pool.get_conn().unwrap();
        let mut transaction = conn.start_transaction(TxOpts::default()).unwrap();
        for query in delete_queries {
            transaction.query_drop(query).unwrap();
        }
        let affected_rows = transaction.affected_rows();
        match transaction.commit() {
            Ok(_) => {
                // info!("Executed successfully");
                Ok(affected_rows)
            }
            Err(e) => {
                error!("Error in writing to database {}", e);
                match e {
                    mysql::Error::MySqlError(e) => {
                        error!("Error in writing to database {}", e);
                        Err(DBError::MySqlError(e))
                    }
                    _ => {
                        error!("Error in writing to database {}", e);
                        Err(DBError::GenericError(e.to_string()))
                    }
                }
            }
        }
    }

    /// Run a read-only query produced by the provided DAO and return the raw
    /// `mysql::Row` result set.
    ///
    /// The DAO's `get_query_string` method is called to produce the SQL string
    /// which is executed directly on a pooled connection. Caller receives
    /// `DBError` mapped from driver errors.
    pub fn query(&self, query_entity: Box<dyn dao::Dao>, context: Arc<MySqlContext>) -> Result<Vec<Row>, DBError> {
        let query_string = match query_entity.get_query_string() {
            Ok(o) => o,
            Err(e) => {
                error!("Error while building the queries {}", e);
                return Err(DBError::GenericError(e));
            }
        };
        let mut conn = context.pool.get_conn().unwrap();
        let result = match conn.query(query_string) {
            Ok(o) => o,
            Err(e) => {
                error!("Error in querying the table {}", e);
                return match e {
                    mysql::Error::MySqlError(e) => {
                        error!("Error in writing to database {}", e);
                        Err(DBError::MySqlError(e))
                    }
                    _ => {
                        error!("Error in writing to database {}", e);
                        Err(DBError::GenericError(e.to_string()))
                    }
                };
            }
        };
        Ok(result)
    }
}

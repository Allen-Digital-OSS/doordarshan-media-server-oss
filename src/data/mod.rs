//! Module to host entities to represent the data model used.

use std::env;
use log::info;
use crate::common;

pub struct MySqlUrl;

const MYSQL_HOST: &'static str = "MYSQL_HOST";
const MYSQL_USER: &'static str = "MYSQL_USER";
const MYSQL_PASSWORD: &'static str = "MYSQL_PASSWORD";
const MYSQL_DATABASE: &'static str = "MYSQL_DATABASE";
const MYSQL_DEFAULT_HOST: &'static str = "127.0.0.1";
const MYSQL_DEFAULT_USER: &'static str = "root";
const MYSQL_DEFAULT_PASSWORD: &'static str = "";
const MYSQL_DEFAULT_DATABASE: &'static str = "doordarshan";
const DATABASE_URL: &'static str = "DATABASE_URL";
impl crate::data::MySqlUrl {
    pub fn new() -> String {
        if env::var(DATABASE_URL).is_ok() {
            info!("Connecting to Cloud SQL");
            return env::var(DATABASE_URL).unwrap();
        }
        else {
            info!("Connecting to Local MySQL");
            "mysql://".to_string() +
                common::get_env(MYSQL_USER, MYSQL_DEFAULT_USER).as_str()
                + ":" +
                common::get_env(MYSQL_PASSWORD, MYSQL_DEFAULT_PASSWORD).as_str() +
                "@" +
                common::get_env(MYSQL_HOST,  MYSQL_DEFAULT_HOST).as_str() +
                "/" +
                common::get_env(MYSQL_DATABASE, MYSQL_DEFAULT_DATABASE).as_str()
        }
    }
}

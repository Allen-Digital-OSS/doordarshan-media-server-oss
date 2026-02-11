//! Data Access Object (DAO) utilities and SQL builders.
//!
//! This module defines the `Dao` trait and a set of concrete types which
//! implement it. Each implementation provides SQL strings for common
//! CRUD operations (insert/update/delete/query) using the
//! `sql_query_builder` crate. The code intentionally separates SQL
//! generation from execution so callers can inspect, log or execute
//! the generated SQL using their choice of DB client.
//!
//! Conventions and notes:
//! - Each DAO struct carries a `mutation_type: MutationType` field which
//!   indicates the operation that the caller intends (Insert, Update, Delete,
//!   Query, or custom query variants). This is used by higher-level code
//!   to decide which SQL statement to run.
//! - All SQL-producing methods return `Result<String, String>` where the
//!   `Err` variant is used to indicate a missing mandatory field or an
//!   unsupported case. Callers should handle these errors before attempting
//!   execution.
//! - SQL fragments are constructed with simple `format!` usage; this module
//!   does not perform parameter binding. For security, callers should prefer
//!   prepared statements and parameterization when executing the generated
//!   SQL with untrusted input.

use log::error;
use std::fmt::Display;
use sql_query_builder::{Delete, Insert, Update, Select};
use sql_query_builder::InsertClause::{InsertInto, ReplaceInto};

/// Type of mutation/operation the DAO represents.
///
/// This enum is used by consumers of `Dao` objects to determine which SQL
/// string to request and execute. The `CustomQuery1` and `CustomQuery2`
/// variants are provided for DAOs that need specialized non-standard
/// operations beyond the basic CRUD set.
#[derive(Clone)]
pub enum MutationType {
    Insert,
    Update,
    Delete,
    Query,
    CustomQuery1,
    CustomQuery2,
}

/// Core trait for DAO objects.
///
/// Implementors produce SQL strings for various operations. Consumers call
/// the appropriate `get_*_string` method according to the `MutationType`.
///
/// Implementations should validate required fields and return `Err(...)`
/// with a human-friendly message when inputs are insufficient.
pub trait Dao: Send {
    /// Return the mutation type this DAO represents.
    fn get_mutation_type(&self) -> MutationType;

    /// Build the SQL string for an INSERT operation.
    ///
    /// Implementations should return `Err` if required fields for insertion
    /// are missing.
    fn get_insert_string(&self) -> Result<String, String>;

    /// Build the SQL string for an UPDATE operation.
    ///
    /// Default implementation returns `Err("Not implemented")`.
    fn get_update_string(&self) -> Result<String, String> {
        Err("Not implemented".to_string())
    }

    /// Build the SQL string for a DELETE operation.
    ///
    /// Default implementation returns `Err("Not implemented")`.
    fn get_delete_string(&self) -> Result<String, String> {
        Err("Not implemented".to_string())
    }

    /// Build the SQL string for a SELECT/QUERY operation.
    ///
    /// Default implementation returns `Err("Not implemented")`.
    fn get_query_string(&self) -> Result<String, String> {
        Err("Not implemented".to_string())
    }

    /// Build a custom query variant 1 (semantics depend on the DAO).
    ///
    /// Default implementation returns `Err("Not implemented")`.
    fn get_custom_query1_string(&self) -> Result<String, String> {
        Err("Not implemented".to_string())
    }

    /// Build a custom query variant 2 (semantics depend on the DAO).
    ///
    /// Default implementation returns `Err("Not implemented")`.
    fn get_custom_query2_string(&self) -> Result<String, String> {
        Err("Not implemented".to_string())
    }
}

/// Records heartbeat information for a container.
///
/// Required on insert: `host`, `tenant`, and `heartbeat`.
/// On update only `heartbeat` is used. `container_id` is always required
/// to identify the row.
pub struct ContainerHeartbeat {
    pub mutation_type: MutationType,
    pub container_id: String,
    pub heartbeat: Option<u128>,
    pub host: Option<String>,
    pub tenant: Option<String>,
}
impl Dao for ContainerHeartbeat {
    fn get_mutation_type(&self) -> MutationType {
        return self.mutation_type.clone();
    }

    fn get_insert_string(&self) -> Result<String, String> {
        if self.host.is_none() || self.tenant.is_none() || self.heartbeat.is_none() {
            return Err("Host and Tenant are mandatory fields".to_string());
        }
        let mut insert_query_builder = Insert::new();
        Ok(insert_query_builder
            .insert_into("container_heartbeat (container_id, heartbeat, host, tenant)")
            .values(format!("('{}',{},'{}','{}')", self.container_id,
                            self.heartbeat.clone().unwrap(),
                            self.host.clone().unwrap(),
                            self.tenant.clone().unwrap()).as_str())
            .as_string())
    }

    fn get_update_string(&self) -> Result<String, String> {
        let mut update_query_builder = Update::new();
        update_query_builder = update_query_builder
            .update("container_heartbeat");
        if self.heartbeat.is_some() {
            let raw_heartbeat_update = format!("heartbeat = {}", self.heartbeat.unwrap());
            let where_clause = format!("container_id = '{}'", self.container_id);
            update_query_builder = update_query_builder
                .set(raw_heartbeat_update.as_str())
                .where_clause(where_clause.as_str());
        } else if self.heartbeat.is_none() {
            return Err("Nothing to update".to_string());
        }
        return Ok(update_query_builder.as_string());
    }
}
/// Mapping between a worker and its container.
///
/// On insert both `container_id` and `worker_id` are required.
pub struct WorkerContainer {
    pub mutation_type: MutationType,
    pub container_id: String,
    pub worker_id: String,
}
impl Dao for WorkerContainer {
    fn get_mutation_type(&self) -> MutationType {
        return self.mutation_type.clone();
    }
    fn get_insert_string(&self) -> Result<String, String> {
        let mut insert_query_builder = Insert::new();
        let insert_query = insert_query_builder
            .insert_into("worker_container (container_id, worker_id)")
            .values(format!("('{}','{}')", self.container_id, self.worker_id).as_str())
            .as_string();
        return Ok(insert_query);
    }
}

/// Meeting status row (enabled/disabled).
///
/// On insert `enabled` is mandatory. `meeting_id` identifies the row for
/// deletes.
pub struct MeetingStatus {
    pub mutation_type: MutationType,
    pub meeting_id: String,
    pub enabled: Option<bool>
}

impl Dao for MeetingStatus{
    fn get_mutation_type(&self) -> MutationType {
        self.mutation_type.clone()
    }
    fn get_insert_string(&self) -> Result<String, String> {
        let mut insert_query_builder = Insert::new();
        if self.enabled.is_none() {
            error!("Enabled is mandatory field");
            return Err("Enabled is mandatory field".to_string());
        }
        let enabled = self.enabled.unwrap().clone();
        Ok(insert_query_builder
            .insert_into("meeting_status (meeting_id, enabled)")
            .values(format!("('{}',{})", self.meeting_id, enabled).as_str())
            .as_string())
    }
    fn get_delete_string(&self) -> Result<String, String> {
        let delete_query_builder = Delete::new();
        Ok(delete_query_builder
            .delete_from("meeting_status")
            .where_clause(format!("meeting_id = '{}'", self.meeting_id).as_str())
            .as_string())
    }
}
/// Relationship between a router and a worker.
///
/// `worker_id` may be `None` for operations that only involve `router_id`.
pub struct RouterWorker {
    pub mutation_type: MutationType,
    pub worker_id: Option<String>,
    pub router_id: String,
}
impl Dao for RouterWorker {
    fn get_mutation_type(&self) -> MutationType {
        self.mutation_type.clone()
    }
    fn get_insert_string(&self) -> Result<String, String> {
        let mut insert_query_builder = Insert::new();
        if self.worker_id.is_none() {
            return Err("Worker Id is mandatory field".to_string());
        }
        let worker_id = self.worker_id.clone().unwrap();
        Ok(insert_query_builder
            .insert_into("router_worker (worker_id, router_id)")
            .values(format!("('{}','{}')", worker_id, self.router_id).as_str())
            .as_string())
    }
    fn get_delete_string(&self) -> Result<String, String> {
        let mut delete_query_builder = Delete::new();
        Ok(delete_query_builder
            .delete_from("router_worker")
            .where_clause(format!("router_id = '{}'", self.router_id).as_str())
            .as_string())
    }
}

/// Mapping of a router to a meeting with capacity information.
///
/// This struct supports inserting, updating, deleting and querying the
/// `router_meeting` table. Either `capacity` or the pair `producer_capacity`
/// / `consumer_capacity` must be provided for inserts/updates.
pub struct RouterMeeting {
    pub mutation_type: MutationType,
    pub router_id: Option<String>,
    pub meeting_id: String,
    pub producer_capacity: Option<i32>,
    pub consumer_capacity: Option<i32>,
    pub capacity: Option<i32>,
    pub container_id: Option<String>,
}
impl Dao for RouterMeeting {
    fn get_mutation_type(&self) -> MutationType {
        return self.mutation_type.clone();
    }


    fn get_insert_string(&self) -> Result<String, String> {
        let mut insert_query_builder = Insert::new();
        if self.router_id.is_none() {
            return Err("Router Id is mandatory field".to_string());
        }
        if self.container_id.is_none() {
            return Err("Container Id is mandatory field".to_string());
        }
        if (self.producer_capacity.is_none() && self.consumer_capacity.is_none()) && self.capacity.is_none() {
            return Err("Producer Capacity and Consumer Capacity or Capacity are mandatory fields".to_string());
        }
        let router_id = self.router_id.clone().unwrap();
        let container_id = self.container_id.clone().unwrap();
        if self.capacity.is_some() {
            Ok(insert_query_builder
                .insert_into("router_meeting (router_id, meeting_id, \
                                        container_id, capacity)")
                .values(format!("('{}','{}','{}',{})", router_id, self.meeting_id,
                                container_id, self.capacity.unwrap()).as_str())
                .as_string())
        } else if self.producer_capacity.is_some() {
            Ok(insert_query_builder
                .insert_into("router_meeting (router_id, meeting_id, \
                                        container_id, producer_capacity)")
                .values(format!("('{}','{}','{}',{})", router_id, self.meeting_id, container_id,
                                self.producer_capacity.unwrap()).as_str())
                .as_string())
        } else {
            Ok(insert_query_builder
                .insert_into("router_meeting (router_id, meeting_id, \
                                        container_id, consumer_capacity)")
                .values(format!("('{}','{}','{}',{})", router_id, self.meeting_id, container_id,
                                self.consumer_capacity.unwrap()).as_str())
                .as_string())
        }
    }

    fn get_update_string(&self) -> Result<String, String> {
        if self.router_id.is_none() {
            return Err("Router Id is mandatory field".to_string());
        }
        let router_id = self.router_id.clone().unwrap();
        if self.capacity.is_some() {
            let update_query = format!("router_id = '{}', meeting_id = '{}', capacity ='{}'",
                                       router_id, self.meeting_id, self.capacity.unwrap());
            Ok(Update::new().update("router_meeting")
                .set(update_query.as_str())
                .where_clause(format!("meeting_id = '{}'", self.meeting_id).as_str()).as_string())
        } else if self.producer_capacity.is_some() && self.consumer_capacity.is_some() {
            let update_query = format!("router_id = '{}', meeting_id = '{}', producer_capacity = '{}', consumer_capacity = '{}'",
                                       router_id, self.meeting_id, self.producer_capacity.unwrap(), self.consumer_capacity.unwrap());
            return Ok(Update::new().update("router_meeting")
                .set(update_query.as_str())
                .where_clause(format!("meeting_id = '{}'", self.meeting_id).as_str()).as_string());
        } else {
            error!("Mandatory fields are missing");
            return Err("Mandatory fields are missing".to_string());
        }
    }

    fn get_delete_string(&self) -> Result<String, String> {
        let mut delete_query_builder = Delete::new();
        if self.router_id.is_none() && self.container_id.is_none() {
            return Ok(delete_query_builder
                .delete_from("router_meeting")
                .where_clause(format!("meeting_id = '{}'", self.meeting_id).as_str())
                .as_string());
        }
        if self.container_id.is_none() {
            let router_id = self.router_id.clone().unwrap();
            return Ok(delete_query_builder
                .delete_from("router_meeting")
                .where_clause(format!("router_id = '{}' and meeting_id = '{}'", router_id, self.meeting_id).as_str())
                .as_string());
        }
        let container_id = self.container_id.clone().unwrap();
        Ok(delete_query_builder
            .delete_from("router_meeting")
            .where_clause(format!("container_id = '{}' and meeting_id = '{}'", container_id, self.meeting_id).as_str())
            .as_string())
    }

    fn get_query_string(&self) -> Result<String, String> {
        let mut query_builder = Select::new();
        Ok(query_builder
            .select("router_id")
            .from("router_meeting")
            .where_clause(format!("meeting_id = '{}'", self.meeting_id).as_str())
            .as_string())
    }
}

/// Participant membership in a meeting and optional instance scoping.
///
/// Insert requires `participant_id` and `instance_id`.
pub struct ParticipantMeeting {
    pub mutation_type: MutationType,
    pub meeting_id: String,
    pub participant_id: Option<String>,
    pub instance_id: Option<String>,
}
impl Dao for ParticipantMeeting {
    fn get_mutation_type(&self) -> MutationType {
        self.mutation_type.clone()
    }
    fn get_insert_string(&self) -> Result<String, String> {
        if self.participant_id.is_none() || self.instance_id.is_none() {
            return Err("Participant Id & Instance Id is mandatory field".to_string());
        }
        let participant_id = self.participant_id.clone().unwrap();
        let instance_id = self.instance_id.clone().unwrap();
        let mut insert_query_builder = Insert::new();
        Ok(insert_query_builder
            .insert_into("participant_meeting (meeting_id, participant_id, instance_id)")
            .values(format!("('{}','{}', '{}')", self.meeting_id, participant_id, instance_id).as_str())
            .as_string())
    }

    fn get_update_string(&self) -> Result<String, String> {
        if self.participant_id.is_none() || self.instance_id.is_none() {
            return Err("Participant Id & Instance Id is mandatory field".to_string());
        }
        let participant_id = self.participant_id.clone().unwrap();
        let instance_id = self.instance_id.clone().unwrap();
        let mut update_query_builder = Update::new();
        Ok(update_query_builder
            .update("participant_meeting")
            .set(format!("instance_id = '{}'", instance_id).as_str())
            .where_clause(format!("participant_id = '{}' and meeting_id = '{}'",
                                  participant_id, self.meeting_id).as_str())
            .as_string())
    }

    fn get_delete_string(&self) -> Result<String, String> {
        let mut delete_query_builder = Delete::new();
        if self.participant_id.is_none() {
            return Err("Participant Id is mandatory field".to_string());
        }
        let participant_id = self.participant_id.clone().unwrap();
        if self.instance_id.is_some() {
            let instance_id = self.instance_id.clone().unwrap();
            Ok(delete_query_builder
                .delete_from("participant_meeting")
                .where_clause(format!("meeting_id = '{}' and participant_id = '{}' and instance_id = '{}'",
                                      self.meeting_id, participant_id, instance_id).as_str())
                .as_string())
        } else {
            Ok(delete_query_builder
                .delete_from("participant_meeting")
                .where_clause(format!("meeting_id = '{}' and participant_id = '{}'",
                                      self.meeting_id, participant_id).as_str())
                .as_string())
        }
    }
    fn get_query_string(&self) -> Result<String, String> {
        let mut query_builder = Select::new();
        Ok(query_builder
            .select("participant_id,meeting_id")
            .from("participant_meeting")
            .where_clause(format!("meeting_id = '{}'", self.meeting_id).as_str())
            .as_string())
    }

    fn get_custom_query1_string(&self) -> Result<String, String> {
        let mut delete_query_builder = Delete::new();
        if self.participant_id.is_none() || self.instance_id.is_none() {
            return Err("Participant Id & Instance Id are mandatory fields".to_string());
        }
        let participant_id = self.participant_id.clone().unwrap();
        let instance_id = self.instance_id.clone().unwrap();
        Ok(delete_query_builder
            .delete_from("participant_meeting")
            .where_clause(format!("meeting_id = '{}' and participant_id = '{}' and instance_id != '{}'",
                                  self.meeting_id, participant_id, instance_id).as_str())
            .as_string())

    }
}

/// Link a participant to the router used for producing media.
///
/// `router_id` is required for inserts.
pub struct ParticipantProducerRouter {
    pub mutation_type: MutationType,
    pub participant_id: String,
    pub router_id: Option<String>,
}

impl Dao for ParticipantProducerRouter {
    fn get_mutation_type(&self) -> MutationType {
        self.mutation_type.clone()
    }

    fn get_insert_string(&self) -> Result<String, String> {
        if self.router_id.is_none() {
            error!("Producer Router Id is mandatory field");
            return Err("Producer Router Id is mandatory field".to_string());
        }
        let insert_query_builder = Insert::new();
        Ok(insert_query_builder
            .insert_into("participant_producer_router (participant_id, router_id)")
            .values(format!("('{}','{}')", self.participant_id, self.router_id.clone().unwrap()).as_str())
            .as_string())
    }
}
pub struct ParticipantConsumerRouter {
    pub mutation_type: MutationType,
    pub participant_id: String,
    pub router_id: Option<String>,
}

impl Dao for ParticipantConsumerRouter {
    fn get_mutation_type(&self) -> MutationType {
        self.mutation_type.clone()
    }

    fn get_insert_string(&self) -> Result<String, String> {
        if self.router_id.is_none() {
            error!("Consumer Router Id is mandatory field");
            return Err("Consumer Router Id is mandatory field".to_string());
        }
        let insert_query_builder = Insert::new();
        Ok(insert_query_builder
            .insert_into("participant_consumer_router (participant_id, router_id)")
            .values(format!("('{}','{}')", self.participant_id, self.router_id.clone().unwrap()).as_str())
            .as_string())
    }
}

/// Attach a consumer transport to a participant.
///
/// `transport_id` is required on insert. Delete removes all consumer
/// transports for the participant.
pub struct ParticipantConsumerTransport {
    pub mutation_type: MutationType,
    pub participant_id: String,
    pub transport_id: Option<String>,
}
impl Dao for ParticipantConsumerTransport {
    fn get_mutation_type(&self) -> MutationType {
        self.mutation_type.clone()
    }

    fn get_insert_string(&self) -> Result<String, String> {
        if self.transport_id.is_none() {
            error!("Transport Id is mandatory field");
            return Err("Transport Id is mandatory field".to_string());
        }
        let insert_query_builder = Insert::new();
        Ok(insert_query_builder
            .insert_into("participant_consumer_transport (participant_id, transport_id)")
            .values(format!("('{}','{}')", self.participant_id, self.transport_id.clone().unwrap()).as_str())
            .as_string())
    }

    fn get_delete_string(&self) -> Result<String, String> {
        let delete_query_builder = Delete::new();
        Ok(delete_query_builder
            .delete_from("participant_consumer_transport")
            .where_clause(format!("participant_id = '{}'", self.participant_id).as_str())
            .as_string())
    }
}

/// Attach a producer transport to a participant.
///
/// `transport_id` is required on insert. Delete removes all producer
/// transports for the participant.
pub struct ParticipantProducerTransport {
    pub mutation_type: MutationType,
    pub participant_id: String,
    pub transport_id: Option<String>,
}

impl Dao for ParticipantProducerTransport {
    fn get_mutation_type(&self) -> MutationType {
        self.mutation_type.clone()
    }

    fn get_insert_string(&self) -> Result<String, String> {
        if self.transport_id.is_none() {
            error!("Transport Id is mandatory field");
            return Err("Transport Id is mandatory field".to_string());
        }
        let insert_query_builder = Insert::new();
        Ok(insert_query_builder
            .insert_into("participant_producer_transport (participant_id, transport_id)")
            .values(format!("('{}','{}')", self.participant_id, self.transport_id.clone().unwrap()).as_str())
            .as_string())
    }

    fn get_delete_string(&self) -> Result<String, String> {
        let delete_query_builder = Delete::new();
        Ok(delete_query_builder
            .delete_from("participant_producer_transport")
            .where_clause(format!("participant_id = '{}'", self.participant_id).as_str())
            .as_string())
    }
}

/// Generic participant producers table helper.
///
/// Many optional fields are supported; callers must ensure mandatory fields
/// (`kind`, `producers`, `producer_id`, `meeting_id`, `participant_id`) are
/// present for inserts. Deletes and queries support several different
/// filtering options as implemented below.
pub struct ParticipantProducers {
    pub mutation_type: MutationType,
    pub participant_id: Option<String>,
    pub producers: Option<i32>,
    pub kind: Option<String>,
    pub producer_id: Option<String>,
    pub meeting_id: Option<String>,
    pub transport_id: Option<String>,
}
impl Dao for ParticipantProducers {
    fn get_mutation_type(&self) -> MutationType {
        self.mutation_type.clone()
    }

    fn get_insert_string(&self) -> Result<String, String> {
        if self.kind.is_none() || self.producers.is_none() || self.producer_id.is_none() || self.meeting_id.is_none() || self.participant_id.is_none() {
            return Err("Mandatory fields are missing".to_string());
        }
        let mut insert_query_builder = Insert::new();
        let kind = self.kind.clone().unwrap();
        let producers = self.producers.clone().unwrap();
        let producer_id = self.producer_id.clone().unwrap();
        let meeting_id = self.meeting_id.clone().unwrap();
        let participant_id = self.participant_id.clone().unwrap();
        let transport_id = self.transport_id.clone().unwrap();
        Ok(insert_query_builder
            .insert_into("participant_producers (participant_id, producers, kind, producer_id, meeting_id, transport_id)")
            .values(format!("('{}',{},'{}','{}','{}','{}')", participant_id, producers, kind, producer_id, meeting_id, transport_id).as_str())
            .as_string())
    }
    fn get_delete_string(&self) -> Result<String, String> {
        let mut delete_query_builder = Delete::new();
        if self.producer_id.is_some() {
            let producer_id = self.producer_id.clone().unwrap();
            let participant_id = self.participant_id.clone().unwrap();
            Ok(delete_query_builder
                .delete_from("participant_producers")
                .where_clause(format!("participant_id = '{}' and producer_id = '{}'", participant_id, producer_id).as_str())
                .as_string())
        } else if self.kind.is_some() {
            let kind = self.kind.clone().unwrap();
            let participant_id = self.participant_id.clone().unwrap();
            Ok(delete_query_builder
                .delete_from("participant_producers")
                .where_clause(format!("participant_id = '{}' and kind = '{}'", participant_id, kind).as_str())
                .as_string())
        } else if self.producer_id.is_some() {
            Ok(delete_query_builder
                .delete_from("participant_producers")
                .where_clause(format!("producer_id = '{}'", self.producer_id.clone().unwrap()).as_str())
                .as_string())
        } else {
            let participant_id = self.participant_id.clone().unwrap();
            Ok(delete_query_builder
                .delete_from("participant_producers")
                .where_clause(format!("participant_id = '{}'", participant_id).as_str())
                .as_string())
        }
    }
    fn get_query_string(&self) -> Result<String, String> {
        let mut query_builder = Select::new();
        if self.meeting_id.is_some() {
            let meeting_id = self.meeting_id.clone().unwrap();
            return Ok(query_builder
                .select("participant_id, producer_id, kind")
                .from("participant_producers")
                .where_clause(format!("meeting_id = '{}'", meeting_id).as_str())
                .as_string());
        }
        Err("Unhandled case".to_string())
    }

    fn get_custom_query2_string(&self) -> Result<String, String> {
        let mut query_builder = Select::new();
        if self.meeting_id.is_some() && self.participant_id.is_some() {
            let meeting_id = self.meeting_id.clone().unwrap();
            let participant_id = self.participant_id.clone().unwrap();
            return Ok(query_builder
                .select("participant_id, producer_id, kind")
                .from("participant_producers")
                .where_clause(format!("meeting_id = '{}' and participant_id != '{}", meeting_id, participant_id).as_str())
                .as_string());
        }
        Err("Unhandled case".to_string())
    }
}
/// Generic participant consumers table helper.
///
/// Supports several delete patterns and requires `consumers`, `target_participant_id`,
/// `kind`, `consumer_id`, `meeting_id` depending on the operation.
pub struct ParticipantConsumers {
    pub mutation_type: MutationType,
    pub participant_id: Option<String>,
    pub consumers: Option<i32>,
    pub target_participant_id: Option<String>,
    pub kind: Option<String>,
    pub consumer_id: Option<String>,
    pub meeting_id: Option<String>,
    pub transport_id: Option<String>,
}
impl Dao for ParticipantConsumers {
    fn get_mutation_type(&self) -> MutationType {
        self.mutation_type.clone()
    }
    fn get_insert_string(&self) -> Result<String, String> {
        let mut insert_query_builder = Insert::new();
        if self.consumers.is_some() && self.target_participant_id.is_some() && self.kind.is_some() && self.consumer_id.is_some() && self.meeting_id.is_some() {
            let kind = self.kind.clone().unwrap();
            let consumers = self.consumers.clone().unwrap();
            let target_participant_id = self.target_participant_id.clone().unwrap();
            let consumer_id = self.consumer_id.clone().unwrap();
            let participant_id = self.participant_id.clone().unwrap();
            let meeting_id = self.meeting_id.clone().unwrap();
            let transport_id = self.transport_id.clone().unwrap();
            Ok(insert_query_builder
                .insert_into("participant_consumers (participant_id, consumers, target_participant_id, kind, consumer_id, meeting_id, transport_id)")
                .values(format!("('{}',{},'{}','{}','{}','{}','{}')", participant_id, consumers, target_participant_id, kind, consumer_id, meeting_id, transport_id).as_str())
                .as_string())
        } else { Err("mandatory fields are missing".to_string()) }
    }
    fn get_delete_string(&self) -> Result<String, String> {
        let mut delete_query_builder = Delete::new();
        if self.kind.is_some() && self.target_participant_id.is_some() && self.participant_id.is_some() {
            let kind = self.kind.clone().unwrap();
            let target_participant_id = self.target_participant_id.clone().unwrap();
            let participant_id = self.participant_id.clone().unwrap();
            Ok(delete_query_builder
                .delete_from("participant_consumers")
                .where_clause(format!("participant_id = '{}' and target_participant_id = '{}' and kind = '{}'",
                                      participant_id, target_participant_id, kind).as_str())
                .as_string())
        } else if self.target_participant_id.is_some() && self.participant_id.is_some() {
            let target_participant_id = self.target_participant_id.clone().unwrap();
            let participant_id = self.participant_id.clone().unwrap();
            Ok(delete_query_builder
                .delete_from("participant_consumers")
                .where_clause(format!("participant_id = '{}' and target_participant_id = '{}'",
                                      participant_id, target_participant_id).as_str())
                .as_string())
        } else if self.target_participant_id.is_some() && self.participant_id.is_none() {
            let target_participant_id = self.target_participant_id.clone().unwrap();
            Ok(delete_query_builder
                .delete_from("participant_consumers")
                .where_clause(format!("target_participant_id = '{}'",
                                      target_participant_id).as_str())
                .as_string())
        } else if self.consumer_id.is_some() {
            let consumer_id = self.consumer_id.clone().unwrap();
            Ok(delete_query_builder
                .delete_from("participant_consumers")
                .where_clause(format!("consumer_id = '{}'", consumer_id).as_str())
                .as_string())
        } else {
            let participant_id = self.participant_id.clone().unwrap();
            Ok(delete_query_builder
                .delete_from("participant_consumers")
                .where_clause(format!("participant_id = '{}'",
                                      participant_id).as_str())
                .as_string())
        }
    }
}

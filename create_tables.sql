create table container_heartbeat (
    container_id varchar(64),
    host varchar(64),
    tenant varchar(64),
    heartbeat bigint(20),
    PRIMARY KEY (container_id)
);
alter table container_heartbeat add index `tenant_index` (`tenant`);

create table container_status (
    container_id varchar(64),
    status TINYINT(1),
    PRIMARY KEY (container_id)
);

create table worker_container (
    container_id varchar(64),
    worker_id varchar(64),
    PRIMARY KEY (worker_id)
);
create index container_id_index_worker_container ON worker_container (container_id);

create table router_worker (
    worker_id varchar(64),
    router_id varchar(64),
    PRIMARY KEY (router_id)
);
create index worker_id_index_router_worker ON router_worker (worker_id);

create table meeting_status (
    meeting_id varchar(64),
    enabled TINYINT(1),
    PRIMARY KEY (meeting_id)
);

create table router_meeting (
    router_id varchar(64),
    meeting_id varchar(64),
    producer_capacity int,
    consumer_capacity int,
    capacity int,
    container_id varchar(64)
);
--alter table router_meeting add constraint meeting_id_unique UNIQUE(meeting_id);

create index router_id_index_router_meeting ON router_meeting (router_id);
create index meeting_id_index_router_meeting ON router_meeting (meeting_id);

create table participant_meeting (
    meeting_id varchar(64),
    participant_id varchar(64),
    instance_id varchar(64),
    PRIMARY KEY (participant_id)
);
create index meeting_id_index_participant_meeting ON participant_meeting (meeting_id);

create table participant_producer_router (
    participant_id varchar(64),
    router_id varchar(64),
    PRIMARY KEY (participant_id)
);

create table participant_consumer_router (
    participant_id varchar(64),
    router_id varchar(64),
    PRIMARY KEY (participant_id)
);

create table participant_producer_transport (
    participant_id varchar(64),
    transport_id varchar(64),
    PRIMARY KEY (participant_id)
);

create table participant_consumer_transport (
    participant_id varchar(64),
    transport_id varchar(64),
    PRIMARY KEY (participant_id)
);

create table participant_producers (
    participant_id varchar(64),
    producers int,
    kind varchar(64),
    producer_id varchar(64),
    meeting_id varchar(64),
    transport_id varchar(64)
);

create table participant_consumers (
    participant_id varchar(64),
    consumers int,
    target_participant_id varchar(64),
    kind varchar(64),
    consumer_id varchar(64),
    meeting_id varchar(64),
    transport_id varchar(64)
);
alter table participant_consumers add constraint uniq_p_t_k unique (participant_id, target_participant_id, kind);

ALTER TABLE router_worker
ADD FOREIGN KEY (worker_id) REFERENCES worker_container (worker_id);

ALTER TABLE router_meeting
ADD FOREIGN KEY (router_id) REFERENCES router_worker (router_id) ON DELETE CASCADE;

ALTER TABLE participant_meeting
ADD FOREIGN KEY (meeting_id) REFERENCES router_meeting (meeting_id) ON DELETE CASCADE;

ALTER TABLE participant_producer_router
ADD FOREIGN KEY (participant_id) REFERENCES participant_meeting (participant_id) ON DELETE CASCADE;

ALTER TABLE participant_producer_router
ADD FOREIGN KEY (router_id) REFERENCES router_worker (router_id) ON DELETE CASCADE;

ALTER TABLE participant_producer_transport
ADD FOREIGN KEY (participant_id) REFERENCES participant_meeting (participant_id) ON DELETE CASCADE;

ALTER TABLE participant_consumer_transport
ADD FOREIGN KEY (participant_id) REFERENCES participant_meeting (participant_id) ON DELETE CASCADE;

ALTER TABLE participant_consumer_router
ADD FOREIGN KEY (participant_id) REFERENCES participant_meeting (participant_id) ON DELETE CASCADE;

ALTER TABLE participant_consumer_router
ADD FOREIGN KEY (router_id) REFERENCES router_worker (router_id) ON DELETE CASCADE;

ALTER TABLE participant_producers
ADD FOREIGN KEY (participant_id) REFERENCES participant_meeting (participant_id) ON DELETE CASCADE;

ALTER TABLE participant_producers
    ADD FOREIGN KEY (transport_id) REFERENCES participant_producer_transport (transport_id) ON DELETE CASCADE;

ALTER TABLE participant_consumers
ADD FOREIGN KEY (participant_id) REFERENCES participant_meeting (participant_id) ON DELETE CASCADE;

ALTER TABLE participant_consumers
    ADD FOREIGN KEY (transport_id) REFERENCES participant_consumer_transport (transport_id) ON DELETE CASCADE;

ALTER TABLE participant_consumers
ADD CONSTRAINT participant_consumers_ibfk_2 FOREIGN KEY (target_participant_id) REFERENCES participant_meeting (participant_id) ON DELETE CASCADE;

ALTER TABLE router_meeting
ADD CONSTRAINT fk_router_meeting_meeting_id FOREIGN KEY (meeting_id) REFERENCES meeting_status(meeting_id) ON DELETE CASCADE;

```markdown
## mod.rs

### setup_routes

This function is broken down into smaller functions, each responsible for setting up routes for a specific module (sfu, room, participant). This makes the code more modular and easier to understand. Each function returns a Router object with the routes for its respective module.

## payload.rs

### TransportOptions

This structure holds all the necessary information required from the server to establish a transport connection on the client. It includes the transport ID, DTLS parameters, ICE candidates, and ICE parameters.

### InitRequestPayload, CreateProducerRequestPayload, UpdateProducerRequestPayload, ConnectProducerTransportRequestPayload, CreateConsumerRequestPayload, UpdateConsumerRequestPayload, ConnectConsumerTransportRequestPayload

These structures represent different types of requests that can be sent from the client to the server. They include information like RTP capabilities, media kind, and RTP parameters.

### RouterPayload, RouterAudioLevelPayload, ProducerPayload, ProducerStatsPayload, ConsumerPayload, ConsumerStatsPayload, PipeTransportPayload, PipeTransportStatsPayload, CreatePipeConsumerForProducerPayload, CreateRemoteProducerOnPipeTransportPayload, PipeProducerToLocalRouterPayload, PipeConsumerPayload, PipeProducerPayload

These structures represent different types of responses that can be sent from the server to the client. They include information about routers, producers, consumers, pipe transports, and their respective stats.

In general, these structures are used to facilitate the communication between the client and the server in a media streaming application. They allow the client to send requests to the server and the server to send responses back to the client in a structured and standardized way.

## sfu/router.rs

### create_router_handler

This function creates a new router with a given participant count. It takes the participant count from the query parameters of the HTTP request. If the participant count is not provided, it returns a bad request error. If the router creation fails, it returns an internal server error. Otherwise, it returns the details of the created router.

### get_router_handler

This function retrieves the details of a router with a given ID. If the router ID is invalid or the router does not exist, it returns an error. Otherwise, it returns the details of the router.

### get_router_active_speaker_handler

This function retrieves the active speaker of a router with a given ID. If the router ID is invalid or the router does not exist, it returns an error. Otherwise, it returns the active speaker.

### get_router_audio_level_handler

This function retrieves the audio levels of a router with a given ID. If the router ID is invalid or the router does not exist, it returns an error. Otherwise, it returns the audio levels.

### get_router_stats_handler

This function retrieves the stats of a router with a given ID. If the router ID is invalid or the router does not exist, it returns an error. Otherwise, it returns the stats.

### delete_router_handler

This function deletes a router with a given ID. If the router ID is invalid or the router does not exist, it returns an error. Otherwise, it deletes the router and returns a success message.

### parse_and_validate_router_id

This helper function parses and validates a router ID. If the router ID is invalid or the router does not exist, it returns an error. Otherwise, it returns the router ID.

### router_response

This helper function generates a router response. It takes a router and its producers and consumers count, and returns a response with the router details.

## sfu/producer.rs

### create_producer_handler

This function creates a new producer for a given router and participant. It takes the router ID and participant ID from the path and query parameters of the HTTP request, respectively. It also takes the producer details from the request body. If any of the parameters are missing or invalid, or if the producer creation fails, it returns an error. Otherwise, it returns the details of the created producer.

### get_producer_handler

This function retrieves the details of a producer with a given ID. If the producer ID is invalid or the producer does not exist, it returns an error. Otherwise, it returns the details of the producer.

### get_producer_stats_handler

This function retrieves the stats of a producer with a given ID. If the producer ID is invalid or the producer does not exist, it returns an error. Otherwise, it returns the stats.

### put_producer_handler

This function updates a producer with a given ID. It takes the producer details from the request body. If the producer ID is invalid or the producer does not exist, or if the update fails, it returns an error. Otherwise, it returns the details of the updated producer.

### connect_producer_handler

This function connects a producer with a given ID. It takes the DTLS parameters from the request body. If the producer ID is invalid or the producer does not exist, or if the connection fails, it returns an error. Otherwise, it returns a success message.

### pause_producer_handler

This function pauses a producer with a given ID. If the producer ID is invalid or the producer does not exist, or if the pause operation fails, it returns an error. Otherwise, it returns a success message.

### resume_producer_handler

This function resumes a producer with a given ID. If the producer ID is invalid or the producer does not exist, or if the resume operation fails, it returns an error. Otherwise, it returns a success message.

### restart_ice_producer_handler

This function restarts ICE for a producer with a given ID. If the producer ID is invalid or the producer does not exist, or if the restart operation fails, it returns an error. Otherwise, it returns a success message.

### delete_producer_handler

This function deletes a producer with a given ID. If the producer ID is invalid or the producer does not exist, or if the delete operation fails, it returns an error. Otherwise, it returns a success message.

## sfu/consumer.rs

### create_consumer_handler

This function creates a new consumer for a given router and producer. It takes the router ID and producer ID from the path parameters of the HTTP request, and the participant ID from the query parameters. It also takes the consumer details from the request body. If any of the parameters are missing or invalid, or if the consumer creation fails, it returns an error. Otherwise, it returns the details of the created consumer.

### get_consumer_handler

This function retrieves the details of a consumer with a given ID. If the consumer ID is invalid or the consumer does not exist, it returns an error. Otherwise, it returns the details of the consumer.

### get_consumer_stats_handler

This function retrieves the stats of a consumer with a given ID. If the consumer ID is invalid or the consumer does not exist, it returns an error. Otherwise, it returns the stats.

### put_consumer_handler

This function updates a consumer with a given ID. It takes the consumer details from the request body. If the consumer ID is invalid or the consumer does not exist, or if the update fails, it returns an error. Otherwise, it returns the details of the updated consumer.

### connect_consumer_handler

This function connects a consumer with a given ID. It takes the DTLS parameters from the request body. If the consumer ID is invalid or the consumer does not exist, or if the connection fails, it returns an error. Otherwise, it returns a success message.

### pause_consumer_handler

This function pauses a consumer with a given ID. If the consumer ID is invalid or the consumer does not exist, or if the pause operation fails, it returns an error. Otherwise, it returns a success message.

### resume_consumer_handler

This function resumes a consumer with a given ID. If the consumer ID is invalid or the consumer does not exist, or if the resume operation fails, it returns an error. Otherwise, it returns a success message.

### restart_ice_consumer_handler

This function restarts ICE for a consumer with a given ID. If the consumer ID is invalid or the consumer does not exist, or if the restart operation fails, it returns an error. Otherwise, it returns a success message.

### delete_consumer_handler

This function deletes a consumer with a given ID. If the consumer ID is invalid or the consumer does not exist, or if the delete operation fails, it returns an error. Otherwise, it returns a success message.

## sfu/pipe.rs
The pipe.rs file contains handlers for managing pipe transports in a media streaming application.

### create_pipe_transport_handler

This function creates a new pipe transport for a given router. If the router ID is invalid or the router does not exist, or if the pipe transport creation fails, it returns an error. Otherwise, it returns the details of the created pipe transport.

### get_pipe_transport_handler

This function retrieves the details of a pipe transport with a given ID. If the pipe transport ID is invalid or the pipe transport does not exist, it returns an error. Otherwise, it returns the details of the pipe transport.

### get_pipe_transport_stats_handler

This function retrieves the stats of a pipe transport with a given ID. If the pipe transport ID is invalid or the pipe transport does not exist, it returns an error. Otherwise, it returns the stats.

### connect_pipe_transport_to_remote_host_handler

This function connects a pipe transport to a remote host. It takes the remote host IP and port from the query parameters. If the pipe transport ID is invalid or the pipe transport does not exist, or if the connection fails, it returns an error. Otherwise, it returns a success message.

### create_pipe_consumer_for_producer_handler

This function creates a new pipe consumer for a given producer. It takes the producer ID from the path parameters and the RTP capabilities from the request body. If any of the parameters are missing or invalid, or if the consumer creation fails, it returns an error. Otherwise, it returns the details of the created consumer.

### create_remote_producer_on_pipe_transport_handler

This function creates a remote producer on a pipe transport. It takes the producer ID and participant ID from the path parameters and the kind and RTP parameters from the request body. If any of the parameters are missing or invalid, or if the producer creation fails, it returns an error. Otherwise, it returns the details of the created producer.

### delete_pipe_transport_handler

This function deletes a pipe transport with a given ID. If the pipe transport ID is invalid or the pipe transport does not exist, or if the delete operation fails, it returns an error. Otherwise, it returns a success message.

### create_remote_producer_on_pipe_transport_handler

This function creates a remote producer on a pipe transport. It takes the producer ID and participant ID from the path parameters and the kind and RTP parameters from the request body. If any of the parameters are missing or invalid, or if the producer creation fails, it returns an error. Otherwise, it returns the details of the created producer.

### delete_pipe_transport_handler

This function deletes a pipe transport with a given ID. If the pipe transport ID is invalid or the pipe transport does not exist, or if the delete operation fails, it returns an error. Otherwise, it returns a success message.

### create_pipe_from_producer_router_to_destination_router_handler

This function creates a pipe from a producer router to a destination router on the same host where a consumer will be created later. If any of the router or producer IDs are invalid or do not exist, or if the pipe creation fails, it returns an error. Otherwise, it returns a success message.

## sfu/mod.rs

This file contains a module that hosts the common SFU (Selective Forwarding Unit) level APIs and some generic code along with the SFU info API handler.

### get_sfu_info_handler

This function retrieves the information about the SFU. It does not take any parameters and returns the SFU's information.

### reset_sfu_handler

This function resets the SFU. It first closes the current SFU and then resets it. After the reset, it retrieves the SFU's information and returns it.

### SFUPathParams

This structure holds the path parameters for the SFU. It includes optional fields for router ID, destination router ID, producer ID, consumer ID, and pipe transport ID.

### SFUQueryParams

This structure holds the query parameters for the SFU. It includes optional fields for participant ID, participant count, router ID, producer ID, consumer ID, remote host IP, and remote host port.

## participant/mod.rs

### create_handler

This function creates a new participant. It takes the room ID and participant ID from the path parameters, and the participant type, display name, and share screen flag from the query parameters. If any of the parameters are missing or invalid, or if the participant creation fails, it returns an error. Otherwise, it returns the details of the created participant.

### get_handler

This function retrieves the details of a participant with a given ID. If the participant ID is invalid or the participant does not exist, it returns an error. Otherwise, it returns the details of the participant.

### connect_handler

This function connects a participant to a room. It takes the room ID and participant ID from the path parameters. If the participant ID is invalid or the participant does not exist, or if the connection fails, it returns an error. Otherwise, it establishes a WebSocket connection and returns a success message.

### create_and_connect_handler

This function creates a new participant and connects them to a room. It takes the room ID and participant ID from the path parameters, and the room type, participant type, display name, and share screen flag from the query parameters. If any of the parameters are missing or invalid, or if the participant creation or connection fails, it returns an error. Otherwise, it establishes a WebSocket connection and returns the details of the created and connected participant.

### ParticipantPathParams

This structure holds the path parameters for the participant APIs. It includes optional fields for room ID and participant ID.

### CreateParticipantQueryParams

This structure holds the query parameters for creating a participant. It includes optional fields for participant type, display name, and share screen flag.

### CreateAndConnectRoomAndParticipantQueryParams

This structure holds the query parameters for creating and connecting a participant. It includes optional fields for room type, participant type, display name, and share screen flag

## participant/socket.rs

This is a module that hosts the WebSocket event loop for a participant's connection.

### ConnectionWebSocketEventLoop

This is a trait that defines the main event loop to process all signals from the client and respond with server-side signals. It has three methods:\
- run: This method starts the signal processing loop. It sends an initial signal to the client to establish a WebRTC transport connection. It then subscribes the participant to the room to start receiving room-wide signals. It also notifies the client about any producers that already exist in the room and about existing share screen participants in the room. Then it enters a loop where it processes internal signals, sends server signals to the client, processes WebSocket signals, and terminates the participant connection when necessary.
- process_internal_signals: This method processes internal signals received. It handles events like saving a producer, saving a consumer, and handling an active speaker.
- process_websocket_signals: This method processes WebSocket signals. It handles different types of messages received from the WebSocket connection, such as text messages, ping messages, pong messages, binary messages, and close messages.

## impl ConnectionWebSocketEventLoop for ParticipantConnection

This is the implementation of the ConnectionWebSocketEventLoop trait for the ParticipantConnection struct. It provides the actual functionality for the methods defined in the trait.

## room/mod.rs

This a module that hosts the room level APIs.

### create_handler

This function creates a new room. It takes the room type and room size from the query parameters. If any of the parameters are missing or invalid, or if the room creation fails, it returns an error. Otherwise, it returns the details of the created room.

### get_handler

This function retrieves the details of a room with a given ID. If the room ID is invalid or the room does not exist, it returns an error. Otherwise, it returns the details of the room.

### RoomPathParams

This structure holds the path parameters for the room APIs. It includes an optional field for room ID.

### CreateRoomQueryParams

This structure holds the query parameters for creating a room. It includes optional fields for room type and room size.

The file also includes a helper function populate_room_type to validate and convert the room type from a string to a RoomType enum. If the room type is missing or invalid, it returns an error

```

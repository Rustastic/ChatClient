use assembler::HighLevelMessageFactory;
use crossbeam_channel::{select_biased, Receiver, Sender};
use messages::{
    client_commands::{ChatClientCommand, ChatClientEvent},
    high_level_messages::Message,
};
use source_routing::Router;
use std::collections::HashMap;
use wg_2024::{
    network::NodeId,
    packet::{NodeType, Packet},
};

mod handle_command;
mod handle_packet;

/// The `ChatClient` struct represents a client in a chat network.
///
/// It is responsible for managing the client's state, handling incoming commands and packets,
/// and comunicating with other `ChatClient`s through a `CommunicationServer`.
///
/// # Methods
///
/// * `new` - Creates a new instance of `ChatClient`.
/// * `run` - Runs the main event loop for the `ChatClient`.
pub struct ChatClient {
    id: NodeId,
    running: bool,
    registered: Option<NodeId>,
    client_list: Vec<NodeId>,
    msgfactory: HighLevelMessageFactory,
    router: Router,
    communication_server_list: Vec<NodeId>,
    message_buffer: Vec<Message>,
    controller_send: Sender<ChatClientEvent>,
    controller_recv: Receiver<ChatClientCommand>,
    packet_recv: Receiver<Packet>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
}

impl ChatClient {
    /// Creates a new instance of `ChatClient`.
    ///
    /// # Arguments
    ///
    /// * `id` - The unique identifier for the `ChatClient`.
    /// * `controller_send` - A `Sender` to send events to the controller.
    /// * `controller_recv` - A `Receiver` to receive commands from the controller.
    /// * `packet_recv` - A `Receiver` to receive packets.
    /// * `packet_send` - A `HashMap` mapping `NodeId` to `Sender` for sending packets.
    ///
    /// # Returns
    ///
    /// A new `ChatClient` instance.
    #[must_use]
    pub fn new(
        id: NodeId,
        controller_send: Sender<ChatClientEvent>,
        controller_recv: Receiver<ChatClientCommand>,
        packet_recv: Receiver<Packet>,
        packet_send: HashMap<NodeId, Sender<Packet>>,
    ) -> Self {
        Self {
            id,
            msgfactory: HighLevelMessageFactory::new(id, NodeType::Client),
            router: Router::new(id, NodeType::Client),
            client_list: Vec::new(),
            message_buffer: Vec::new(),
            controller_send,
            controller_recv,
            packet_recv,
            packet_send,
            running: false,
            registered: None,
            communication_server_list: Vec::new(),
        }
    }

    /// Runs the main event loop for the `ChatClient`.
    ///
    /// This function continuously listens for incoming commands and packets,
    /// and processes them accordingly. It uses a biased select to prioritize
    /// receiving commands over packets.
    ///
    /// # Panics
    ///
    /// This function will run indefinitely and does not return under normal
    /// operation. It will only stop if the program is terminated.
    pub fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.controller_recv) -> command => {
                    if let Ok(command) = command {
                        self.handle_command(command);
                    }
                },

                recv(self.packet_recv) -> packet => {
                    if let Ok(packet) = packet {
                        self.handle_packet(&packet);
                    }
                },

            }
        }
    }
}

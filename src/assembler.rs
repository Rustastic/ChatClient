use std::collections::HashMap;

use crossbeam_channel::Sender;
use wg_2024::{
    network::NodeId,
    packet::{Fragment, Packet},
};

pub struct Assembler<M> {
    msg_send: Sender<M>,
    packet_send: HashMap<NodeId, Vec<Packet>>,
    fragment_buffer: HashMap<u64, Vec<Fragment>>,
}

impl<M> Assembler<M> {
    pub fn new(
        msg_send: Sender<M>,
        packet_send: HashMap<NodeId, Vec<Packet>>,
        fragment_buffer: HashMap<u64, Vec<Fragment>>,
    ) -> Self {
        Self {
            msg_send,
            packet_send,
            fragment_buffer
        }
    }
}

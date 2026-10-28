//! The in-memory scan result. Owned by the UI thread only; scanner threads send batches.

mod tree;

pub use tree::{Entry, Kind, NONE, Node, NodeId, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tag {
    Red,
    Yellow,
    Green,
    Blue,
}

impl Tag {
    pub const ALL: [Tag; 4] = [Tag::Red, Tag::Yellow, Tag::Green, Tag::Blue];

    pub fn index(self) -> usize {
        self as usize
    }
}

//! Engine-independent domain types. Money is an integer number of minor units.

mod account;
mod agent;
mod config;
mod event;
mod external;
mod institutions;
mod interaction;
mod memory;
mod resource;
mod scenario;

pub use account::*;
pub use agent::*;
pub use config::*;
pub use event::*;
pub use external::*;
pub use institutions::*;
pub use interaction::*;
pub use memory::*;
pub use resource::*;
pub use scenario::*;

/// A domain failure; an invalid intent never authorizes an unchecked mutation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// A configuration value is outside the supported range.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    /// The referenced stable agent identifier does not exist.
    #[error("unknown agent {0}")]
    UnknownAgent(AgentId),
    /// An identifier is duplicated.
    #[error("duplicate agent {0}")]
    DuplicateAgent(AgentId),
    /// The account cannot afford a reservation or debit.
    #[error("insufficient unreserved money")]
    InsufficientFunds,
    /// An inventory cannot satisfy a reservation or withdrawal.
    #[error("insufficient unreserved {0}")]
    InsufficientInventory(Resource),
    /// An action violates a rule, including zero quantity or price.
    #[error("invalid action: {0}")]
    InvalidAction(String),
    /// A checked integer operation exceeded its representation.
    #[error("arithmetic overflow: {0}")]
    Overflow(&'static str),
    /// An internal accounting invariant has been violated.
    #[error("invariant violation: {0}")]
    Invariant(String),
}

/// Domain result shared by the independent engine crates.
pub type Result<T> = std::result::Result<T, DomainError>;

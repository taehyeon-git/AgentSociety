use crate::{DomainError, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Number of resource kinds; array ordering is part of the file format.
pub const RESOURCE_COUNT: usize = 4;
/// Integer minor currency units (100 minor units = 1 credit).
pub type Money = u64;
/// Simulation time, unrelated to wall-clock time.
pub type Tick = u64;
/// Prices in Food, Wood, Metal, Goods order.
pub type Prices = [Money; RESOURCE_COUNT];
/// Aggregate quantities, using wider integers than individual inventories.
pub type ResourceTotals = [u128; RESOURCE_COUNT];

/// Resources with stable serialization names and deterministic iteration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Resource {
    /// Essential consumption and agricultural output.
    Food = 0,
    /// Renewable raw material.
    Wood = 1,
    /// Raw material from a replenished abstraction of regional mining capacity.
    Metal = 2,
    /// Manufactured consumption goods.
    Goods = 3,
}

impl Resource {
    /// Canonical order used for matching, serialization, and statistics.
    pub const ALL: [Self; RESOURCE_COUNT] = [Self::Food, Self::Wood, Self::Metal, Self::Goods];

    /// Index in a resource quantity or price array.
    pub const fn index(self) -> usize {
        self as usize
    }
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Food => "food",
            Self::Wood => "wood",
            Self::Metal => "metal",
            Self::Goods => "goods",
        })
    }
}

/// Nonnegative resource counts. Mutations use checked arithmetic.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Inventory([u64; RESOURCE_COUNT]);

impl Inventory {
    /// Construct counts in the canonical Food, Wood, Metal, Goods order.
    pub const fn new(amounts: [u64; RESOURCE_COUNT]) -> Self {
        Self(amounts)
    }

    /// Read one resource count.
    pub const fn get(&self, resource: Resource) -> u64 {
        self.0[resource.index()]
    }

    /// Copy the fixed-size quantities.
    pub const fn amounts(&self) -> [u64; RESOURCE_COUNT] {
        self.0
    }

    /// Add stock, leaving the inventory unchanged on overflow.
    pub fn add(&mut self, resource: Resource, quantity: u64) -> Result<()> {
        let slot = &mut self.0[resource.index()];
        *slot = slot
            .checked_add(quantity)
            .ok_or(DomainError::Overflow("inventory"))?;
        Ok(())
    }

    /// Remove stock, leaving the inventory unchanged on shortage.
    pub fn remove(&mut self, resource: Resource, quantity: u64) -> Result<()> {
        let slot = &mut self.0[resource.index()];
        *slot = slot
            .checked_sub(quantity)
            .ok_or(DomainError::InsufficientInventory(resource))?;
        Ok(())
    }
}

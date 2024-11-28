use crate::error::Error;

/// A wrapper over a `u64` value which ensures that the inner value is within
/// a the range: 0..100.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct Ranged(pub(crate) u64);

impl Ranged {
    /// Create a new ranged `Self` which is used to ensure that the inner `val` is within
    /// the range of 0 .. 100.
    pub fn new(value: u64) -> Result<Self, Error> {
        if (0..100).contains(&value) {
            Ok(Self(value))
        } else {
            Err(Error::Bounds {})
        }
    }
    /// Create a new ranged `Self` which is used to ensure that the inner `val` is within
    /// the range of 0 .. 100.
    /// If the value is not is within range this will fail to compile.
    pub const fn new_const(value: u64) -> Self {
        if value <= 100 {
            Self(value)
        } else {
            panic!("Not valid")
        }
    }
    /// Create a new `Self` from the given value.
    /// # Warning: the provided value must be within range, otherwise it will be bounded.
    pub(crate) fn new_ranged(value: u64) -> Self {
        Self(value.min(100))
    }
    /// Get the inner u64 value of this Ranged wrapper.
    /// The value is guaranteed to be within 0 .. 100.
    pub fn val(&self) -> u64 {
        self.0
    }
}

impl TryFrom<u8> for Ranged {
    type Error = Error;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value as u64)
    }
}
impl TryFrom<u64> for Ranged {
    type Error = Error;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

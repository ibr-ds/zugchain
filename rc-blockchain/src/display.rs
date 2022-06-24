use std::fmt::{self, Display, Formatter};

use bytes::Bytes;
use fmt::Debug;

const PRINT_CAP: usize = 4;
#[derive(Ord, PartialOrd, Eq, PartialEq)]
pub struct DisplayBytes<'a, T = Bytes>(pub &'a T)
where
    T: ?Sized;

impl<'a, T> Debug for DisplayBytes<'a, T>
where
    T: AsRef<[u8]> + ?Sized,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for c in self.0.as_ref() {
            write!(f, "{:02x}", c)?;
        }
        Ok(())
    }
}

impl<'a, T> Display for DisplayBytes<'a, T>
where
    T: AsRef<[u8]> + ?Sized,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if !self.0.as_ref().is_empty() {
            for c in &self.0.as_ref()[..PRINT_CAP] {
                write!(f, "{:02x}", c)?;
            }
        } else {
            write!(f, "()")?;
        }
        Ok(())
    }
}

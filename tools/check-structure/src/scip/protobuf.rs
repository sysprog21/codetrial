//! Protocol Buffers' wire format, as far as reading SCIP needs it.

use std::fmt;

#[derive(Debug, PartialEq)]
pub enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

#[derive(Debug)]
pub struct Malformed(pub(super) &'static str);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed protobuf: {}", self.0)
    }
}

/// A message's fields in the order they were written: (field number, value).
pub struct Fields<'a> {
    bytes: &'a [u8],
}

impl<'a> Fields<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Fields { bytes }
    }

    fn next_field(&mut self) -> Result<(u32, Value<'a>), Malformed> {
        let key = read_varint(&mut self.bytes)?;
        let number = u32::try_from(key >> 3).map_err(|_| Malformed("field number"))?;
        let value = match key & 7 {
            0 => Value::Varint(read_varint(&mut self.bytes)?),
            1 => {
                self.skip(8)?;
                Value::Fixed
            }
            2 => {
                let length = read_varint(&mut self.bytes)?;
                let length = usize::try_from(length).map_err(|_| Malformed("length"))?;
                Value::Bytes(self.skip(length)?)
            }
            5 => {
                self.skip(4)?;
                Value::Fixed
            }
            _ => return Err(Malformed("wire type")),
        };
        Ok((number, value))
    }

    fn skip(&mut self, length: usize) -> Result<&'a [u8], Malformed> {
        if length > self.bytes.len() {
            return Err(Malformed("truncated"));
        }
        let (taken, rest) = self.bytes.split_at(length);
        self.bytes = rest;
        Ok(taken)
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = Result<(u32, Value<'a>), Malformed>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.is_empty() {
            return None;
        }
        let field = self.next_field();
        if field.is_err() {
            self.bytes = &[];
        }
        Some(field)
    }
}

fn read_varint(bytes: &mut &[u8]) -> Result<u64, Malformed> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes.split_first().ok_or(Malformed("truncated varint"))?;
        *bytes = rest;
        // The tenth byte holds the last bit; anything more overflows.
        if shift == 63 && byte > 1 {
            return Err(Malformed("varint too long"));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok(value);
        }
    }
    Err(Malformed("varint too long"))
}

/// A repeated integer field's values: one value, or a packed run of them.
pub fn integers(value: Value<'_>, into: &mut Vec<u64>) -> Result<(), Malformed> {
    match value {
        Value::Varint(v) => into.push(v),
        Value::Bytes(mut packed) => {
            while !packed.is_empty() {
                into.push(read_varint(&mut packed)?);
            }
        }
        Value::Fixed => return Err(Malformed("fixed-width integer where a varint belongs")),
    }
    Ok(())
}

pub fn string(value: Value<'_>) -> Result<&str, Malformed> {
    match value {
        Value::Bytes(bytes) => {
            std::str::from_utf8(bytes).map_err(|_| Malformed("string is not UTF-8"))
        }
        _ => Err(Malformed("string field is not length-delimited")),
    }
}

pub fn bytes(value: Value<'_>) -> Result<&[u8], Malformed> {
    match value {
        Value::Bytes(bytes) => Ok(bytes),
        _ => Err(Malformed("message field is not length-delimited")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_varints_strings_and_packed_runs() {
        // field 1: varint 300; field 2: "hi"; field 3: packed [1, 150].
        let message = [
            0x08, 0xac, 0x02, 0x12, 2, b'h', b'i', 0x1a, 3, 0x01, 0x96, 0x01,
        ];
        let fields: Vec<_> = Fields::new(&message).collect::<Result<_, _>>().unwrap();
        assert_eq!(fields[0], (1, Value::Varint(300)));
        assert_eq!(string(Value::Bytes(b"hi")).unwrap(), "hi");
        let mut run = Vec::new();
        let (number, packed) = fields.into_iter().nth(2).unwrap();
        assert_eq!(number, 3);
        integers(packed, &mut run).unwrap();
        assert_eq!(run, [1, 150]);
    }

    #[test]
    fn a_varint_past_sixty_four_bits_is_an_error() {
        let mut longest = [0xff; 9].to_vec();
        longest.push(0x01);
        assert_eq!(read_varint(&mut longest.as_slice()).unwrap(), u64::MAX);
        let mut overflowing = [0xff; 9].to_vec();
        overflowing.push(0x02);
        assert!(read_varint(&mut overflowing.as_slice()).is_err());
    }

    #[test]
    fn a_truncated_message_is_an_error() {
        let message = [0x12, 5, b'h'];
        assert!(Fields::new(&message).next().unwrap().is_err());
    }
}

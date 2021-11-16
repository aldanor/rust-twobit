//! Extraction of data from 2bit files

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::mem::size_of;
use std::path::Path;

use crate::block::Block;
use crate::error::{Error, Result};
use crate::types::{Field, FileIndex};
use crate::{REV_SIGNATURE, SIGNATURE};

const FIELD_SIZE: usize = size_of::<Field>();

pub trait Reader: Read + Seek {}

impl<T: Read + Seek> Reader for T {}

/// Extract binary data from 2bit files
///
/// This reads all types of fields except the sequences.
pub struct ValueReader<R: Reader> {
    reader: R,
    twobit_version: Field,
    swap_endian: bool,
}

pub type BoxValueReader = ValueReader<Box<dyn Reader>>;

impl ValueReader<BufReader<File>> {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::new(BufReader::new(File::open(path)?))
    }
}

impl<T> ValueReader<Cursor<T>>
where
    Cursor<T>: Read + Seek,
{
    pub fn from_buf(buf: T) -> Result<Self> {
        Self::new(Cursor::new(buf))
    }
}

impl ValueReader<Cursor<Vec<u8>>> {
    pub fn open_and_read<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut buf = vec![];
        File::open(path)?.read_to_end(&mut buf)?;
        Self::from_buf(buf)
    }
}

impl<R: Reader> ValueReader<R> {
    pub fn new(reader: R) -> Result<Self> {
        let mut result = Self {
            reader,
            twobit_version: 0,
            swap_endian: false,
        };
        let signature = result.field()?;
        if signature != SIGNATURE {
            if signature == REV_SIGNATURE {
                result.swap_endian = true;
            } else {
                return Err(Error::FileFormat(
                    "File does not start with 2bit signature".to_string(),
                ));
            }
        }
        let version = result.field()?;
        if version == 0 {
            result.twobit_version = version;
            Ok(result)
        } else {
            Err(Error::UnsupportedVersion(
                "Versions larger than 0 are not supported".to_string(),
            ))
        }
    }

    /// Box the reader (useful for type erasure if using multiple reader types).
    pub fn boxed(self) -> BoxValueReader
    where
        R: 'static,
    {
        ValueReader {
            reader: Box::new(self.reader),
            twobit_version: self.twobit_version,
            swap_endian: self.swap_endian,
        }
    }

    pub fn seek(&mut self, pos: SeekFrom) -> Result<FileIndex> {
        match self.reader.seek(pos) {
            Ok(v) => Ok(v as FileIndex),
            Err(e) => Err(e.into()),
        }
    }

    pub fn seek_start(&mut self) -> Result<()> {
        self.seek(SeekFrom::Start(2 * size_of::<Field>() as u64))?;
        Ok(())
    }

    pub fn tell(&mut self) -> Result<FileIndex> {
        match self.reader.seek(SeekFrom::Current(0)) {
            Ok(v) => Ok(v as FileIndex),
            Err(e) => Err(e.into()),
        }
    }

    pub fn stream_len(&mut self) -> Result<u64> {
        // borrowed from unstable Seek method in stdlib
        let old_pos = self.reader.stream_position()?;
        let len = self.reader.seek(SeekFrom::End(0))?;
        // Avoid seeking a third time when we were already at the end of the
        // stream. The branch is usually way cheaper than a seek operation.
        if old_pos != len {
            self.reader.seek(SeekFrom::Start(old_pos))?;
        }
        Ok(len)
    }

    pub fn byte(&mut self) -> Result<u8> {
        let mut byte_slice: [u8; 1] = [0; 1];
        self.fill_completely(&mut byte_slice)?;
        Ok(byte_slice[0])
    }

    pub fn field(&mut self) -> Result<Field> {
        let mut field: [u8; FIELD_SIZE] = [0; FIELD_SIZE];
        self.fill_completely(&mut field)?;
        Ok(slice_to_field(field, self.swap_endian))
    }

    pub fn string(&mut self, length: usize) -> Result<String> {
        let mut buf = vec![0_u8; length];
        self.fill_completely(&mut buf)?;
        Ok(String::from_utf8(buf)?)
    }

    pub fn blocks(&mut self) -> Result<Vec<Block>> {
        let num_blocks = self.field()? as usize;
        let mut result = Vec::with_capacity(num_blocks);
        for _ in 0..num_blocks {
            result.push(Block {
                start: self.field()?,
                length: 0, // will be assigned in the next loop
            });
        }

        for block in &mut result {
            block.length = self.field()?;
        }
        Ok(result)
    }

    pub fn skip_blocks(&mut self) -> Result<()> {
        let num_blocks = self.field()? as usize;
        let skip = num_blocks * 2 * size_of::<Field>();
        self.reader.seek(SeekFrom::Current(skip as i64))?;
        Ok(())
    }

    /// Read bytes from the reader until the buffer is completely full
    fn fill_completely(&mut self, buf: &mut [u8]) -> Result<()> {
        let n_bytes = buf.len();
        let mut bytes_read = 0;
        while bytes_read < n_bytes {
            // our reader doesn't guarantee that it's always reading enough bytes at once
            bytes_read += self.reader.read(&mut buf[bytes_read..])?;
        }
        assert_eq!(bytes_read, n_bytes);
        Ok(())
    }
}

fn slice_to_field(slice: [u8; FIELD_SIZE], swap_endian: bool) -> Field {
    let mut result = 0;
    if swap_endian {
        for byte in slice.iter().rev().copied() {
            result <<= 8;
            result += Field::from(byte);
        }
    } else {
        for byte in slice {
            result <<= 8;
            result += Field::from(byte);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slice_to_field() {
        let slice: [u8; 4] = [0x1A, 0x41, 0x27, 0x43];
        assert_eq!(slice_to_field(slice, false), 0x1A412743);
        let slice: [u8; 4] = [0x43, 0x27, 0x41, 0x1A];
        assert_eq!(slice_to_field(slice, false), 0x4327411A);

        assert_eq!(
            slice_to_field([0x1A, 0x41, 0x27, 0x43], true),
            slice_to_field([0x43, 0x27, 0x41, 0x1A,], false)
        );

        let slice: [u8; 4] = [0, 2, 3, 4];
        assert_eq!(slice_to_field(slice, false), 2 * 65536 + 3 * 256 + 4);
    }
}

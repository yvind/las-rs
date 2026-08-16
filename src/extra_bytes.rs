//! Typed access to user-defined attributes stored after LAS point records.
//!
//! Extra Bytes are described by the `LASF_Spec` record 4 VLR. The parser
//! supports data types 0 through 10; deprecated array types and reserved types
//! are rejected because their typed layout is not part of this API.

use std::{marker::PhantomData, slice::ChunksExact};

use crate::{Error, Header, Point, PointData, Result, Vlr};

const NO_DATA_BIT: u8 = 1 << 0;
const MIN_BIT: u8 = 1 << 1;
const MAX_BIT: u8 = 1 << 2;
const SCALE_BIT: u8 = 1 << 3;
const OFFSET_BIT: u8 = 1 << 4;

/// The storage type declared by an Extra Bytes descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtraBytesDataType {
    /// Raw bytes; the descriptor's options byte is their on-disk width.
    Undocumented,
    /// An unsigned 8-bit integer.
    U8,
    /// A signed 8-bit integer.
    I8,
    /// An unsigned 16-bit integer.
    U16,
    /// A signed 16-bit integer.
    I16,
    /// An unsigned 32-bit integer.
    U32,
    /// A signed 32-bit integer.
    I32,
    /// An unsigned 64-bit integer.
    U64,
    /// A signed 64-bit integer.
    I64,
    /// A 32-bit IEEE floating-point number.
    F32,
    /// A 64-bit IEEE floating-point number.
    F64,
    /// A deprecated data type (codes 11 through 30).
    Deprecated(u8),
    /// A reserved data type (codes 31 through 255).
    Reserved(u8),
}

impl ExtraBytesDataType {
    /// Returns the numeric code stored in the descriptor.
    pub fn code(self) -> u8 {
        match self {
            Self::Undocumented => 0,
            Self::U8 => 1,
            Self::I8 => 2,
            Self::U16 => 3,
            Self::I16 => 4,
            Self::U32 => 5,
            Self::I32 => 6,
            Self::U64 => 7,
            Self::I64 => 8,
            Self::F32 => 9,
            Self::F64 => 10,
            Self::Deprecated(code) | Self::Reserved(code) => code,
        }
    }

    /// Returns true for one of the ten supported scalar numeric types.
    pub fn is_scalar(self) -> bool {
        matches!(
            self,
            Self::U8
                | Self::I8
                | Self::U16
                | Self::I16
                | Self::U32
                | Self::I32
                | Self::U64
                | Self::I64
                | Self::F32
                | Self::F64
        )
    }

    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Undocumented,
            1 => Self::U8,
            2 => Self::I8,
            3 => Self::U16,
            4 => Self::I16,
            5 => Self::U32,
            6 => Self::I32,
            7 => Self::U64,
            8 => Self::I64,
            9 => Self::F32,
            10 => Self::F64,
            11..=30 => Self::Deprecated(code),
            31..=255 => Self::Reserved(code),
        }
    }

    fn scalar_size(self) -> Option<usize> {
        match self {
            Self::U8 | Self::I8 => Some(1),
            Self::U16 | Self::I16 => Some(2),
            Self::U32 | Self::I32 | Self::F32 => Some(4),
            Self::U64 | Self::I64 | Self::F64 => Some(8),
            Self::Undocumented | Self::Deprecated(_) | Self::Reserved(_) => None,
        }
    }

    fn byte_size(self, options: u8) -> Option<usize> {
        self.scalar_size().or_else(|| match self {
            Self::Undocumented => Some(usize::from(options)),
            Self::Deprecated(code @ 11..=20) => {
                Self::from_code(code - 10).scalar_size().map(|bs| bs * 2)
            }
            Self::Deprecated(code @ 21..=30) => {
                Self::from_code(code - 20).scalar_size().map(|bs| bs * 3)
            }
            Self::Reserved(_) => None,
            _ => None,
        })
    }
}

/// A numeric Extra Bytes value.
///
/// Unscaled unsigned integers are represented as `Unsigned`, unscaled signed
/// integers as `Signed`, and floating-point or transformed values as `Float`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExtraBytesValue {
    /// An unsigned 64-bit integer.
    Unsigned(u64),
    /// A signed 64-bit integer.
    Signed(i64),
    /// A 64-bit IEEE floating-point number.
    Float(f64),
}

impl ExtraBytesValue {
    /// Converts this value to `f64`.
    pub fn as_f64(self) -> f64 {
        match self {
            Self::Unsigned(value) => value as f64,
            Self::Signed(value) => value as f64,
            Self::Float(value) => value,
        }
    }
}

/// Metadata for one attribute in an Extra Bytes VLR.
#[derive(Clone, Debug)]
pub struct ExtraBytesDescriptor {
    data_type: ExtraBytesDataType,
    options: u8,
    name: String,
    no_data: Option<ExtraBytesValue>,
    min: Option<ExtraBytesValue>,
    max: Option<ExtraBytesValue>,
    scale: f64,
    offset: f64,
    description: String,
    byte_offset: usize,
    byte_size: usize,
}

impl ExtraBytesDescriptor {
    /// Returns the attribute's unique name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the attribute's description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the attribute's storage type.
    pub fn data_type(&self) -> ExtraBytesDataType {
        self.data_type
    }

    /// Returns the raw options byte.
    pub fn options(&self) -> u8 {
        self.options
    }

    /// Returns the offset within a point's Extra Bytes region.
    pub fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    /// Returns the number of bytes occupied in each point record.
    pub fn byte_size(&self) -> usize {
        self.byte_size
    }

    /// Returns the exact no-data value when its option bit is set.
    pub fn no_data(&self) -> Option<ExtraBytesValue> {
        self.no_data
    }

    /// Returns the exact minimum value when its option bit is set.
    pub fn min(&self) -> Option<ExtraBytesValue> {
        self.min
    }

    /// Returns the exact maximum value when its option bit is set.
    pub fn max(&self) -> Option<ExtraBytesValue> {
        self.max
    }

    /// Returns the scale, or one when its option bit is clear.
    pub fn scale(&self) -> f64 {
        if self.data_type.is_scalar() && self.options & SCALE_BIT != 0 {
            self.scale
        } else {
            1.0
        }
    }

    /// Returns the offset, or zero when its option bit is clear.
    pub fn offset(&self) -> f64 {
        if self.data_type.is_scalar() && self.options & OFFSET_BIT != 0 {
            self.offset
        } else {
            0.0
        }
    }

    fn decode(&self, bytes: &[u8]) -> Option<ExtraBytesValue> {
        let value = decode_value(self.data_type, bytes)?;
        if self.options & (SCALE_BIT | OFFSET_BIT) != 0 {
            Some(ExtraBytesValue::Float(
                value.as_f64() * self.scale() + self.offset(),
            ))
        } else {
            Some(value)
        }
    }

    fn is_no_data(&self, value: ExtraBytesValue) -> bool {
        self.no_data == Some(value)
    }
}

/// Parsed Extra Bytes metadata and typed accessors for points and byte slabs.
#[derive(Clone, Debug)]
pub struct ExtraBytesVlr {
    descriptors: Vec<ExtraBytesDescriptor>,
    extra_bytes_len: usize,
    described_bytes_len: usize,
}

impl TryFrom<&Vlr> for ExtraBytesVlr {
    type Error = Error;

    fn try_from(value: &Vlr) -> Result<Self> {
        if !value.is_extra_bytes() {
            return Err(Error::NotExtraBytesVlr);
        }
        Self::from_payload(value.data.as_slice())
    }
}

impl Vlr {
    /// Returns true if this is the LASF_Spec Extra Bytes VLR.
    pub fn is_extra_bytes(&self) -> bool {
        self.user_id == ExtraBytesVlr::USER_ID && self.record_id == ExtraBytesVlr::RECORD_ID
    }
}

impl ExtraBytesVlr {
    /// The registered user ID of the Extra Bytes VLR.
    pub const USER_ID: &'static str = "LASF_Spec";
    /// The record ID of the Extra Bytes VLR.
    pub const RECORD_ID: u16 = 4;
    /// The encoded size of one Extra Bytes descriptor.
    pub const DESCRIPTOR_SIZE: usize = 192;

    /// Parses Extra Bytes descriptors from a header.
    pub fn new(header: &Header) -> Result<Option<Self>> {
        let matching: Vec<_> = header
            .all_vlrs()
            .filter(|vlr| vlr.is_extra_bytes())
            .collect();

        if matching.is_empty() {
            return Ok(None);
        } else if matching.len() > 1 {
            return Err(Error::MultipleExtraBytesVlrs(matching.len()));
        }

        Some(Self::from_payload(matching[0].data.as_slice())).transpose()
    }

    fn from_payload(data: &[u8]) -> Result<Self> {
        if !data.len().is_multiple_of(Self::DESCRIPTOR_SIZE) {
            return Err(Error::InvalidExtraBytesVlrLength(data.len()));
        }
        let mut descriptors = Vec::new();
        let mut described_bytes_len = 0;
        for bytes in data.chunks_exact(Self::DESCRIPTOR_SIZE) {
            let descriptor = parse_descriptor(bytes, described_bytes_len)?;
            described_bytes_len += descriptor.byte_size;
            descriptors.push(descriptor);
        }
        let actual_extra_len = described_bytes_len;
        if described_bytes_len > actual_extra_len {
            return Err(Error::ExtraBytesMismatch(
                described_bytes_len,
                actual_extra_len,
            ));
        }
        Ok(Self {
            descriptors,
            extra_bytes_len: actual_extra_len,
            described_bytes_len,
        })
    }

    /// Returns true when the point format contains trailing Extra Bytes.
    pub fn has_extra_bytes(&self) -> bool {
        self.extra_bytes_len != 0
    }

    /// Returns true when at least one descriptor was parsed.
    pub fn has_descriptors(&self) -> bool {
        !self.descriptors.is_empty()
    }

    /// Returns all descriptors in on-disk order.
    pub fn descriptors(&self) -> &[ExtraBytesDescriptor] {
        &self.descriptors
    }

    /// Returns a descriptor by name.
    pub fn descriptor(&self, name: &str) -> Option<&ExtraBytesDescriptor> {
        self.descriptors.iter().find(|d| d.name() == name)
    }

    /// Returns descriptor names in on-disk order.
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.descriptors.iter().map(|descriptor| &descriptor.name)
    }

    /// Returns descriptor descriptions in on-disk order.
    pub fn descriptions(&self) -> impl Iterator<Item = &String> {
        self.descriptors
            .iter()
            .map(|descriptor| &descriptor.description)
    }

    /// Returns the total Extra Bytes width per point.
    pub fn extra_bytes_len(&self) -> usize {
        self.extra_bytes_len
    }

    /// Returns the bytes covered by descriptors.
    pub fn described_bytes_len(&self) -> usize {
        self.described_bytes_len
    }

    /// Returns the undescribed trailing bytes per point.
    pub fn undocumented_bytes_len(&self) -> usize {
        self.extra_bytes_len - self.described_bytes_len
    }

    /// Returns a numeric column from a byte slab.
    ///
    /// Integer values are widened to `U64` or `I64`. Floating-point values and
    /// values with scale or offset enabled are returned as `F64`.
    pub fn column<'pointdata, 'vlr>(
        &'vlr self,
        name: &str,
        points: &'pointdata PointData,
    ) -> Result<ExtraBytesColumn<'pointdata, 'vlr>> {
        self.ensure_point_data(points)?;
        let descriptor = self.descriptor_or_error(name)?;
        if !descriptor.data_type.is_scalar() {
            return Err(Error::NonNumericExtraBytesField(name.to_owned()));
        }
        let offset = points.record_len() - self.extra_bytes_len + descriptor.byte_offset;
        let records = points.raw_bytes().chunks_exact(points.record_len());
        if descriptor.options & (SCALE_BIT | OFFSET_BIT) != 0 {
            Ok(ExtraBytesColumn::Float(ExtraBytesTypedColumn {
                records,
                record_field_offset: offset,
                descriptor,
                item: PhantomData::<f64>,
            }))
        } else {
            match descriptor.data_type {
                ExtraBytesDataType::U8
                | ExtraBytesDataType::U16
                | ExtraBytesDataType::U32
                | ExtraBytesDataType::U64 => {
                    Ok(ExtraBytesColumn::Unsigned(ExtraBytesTypedColumn {
                        records,
                        record_field_offset: offset,
                        descriptor,
                        item: PhantomData::<u64>,
                    }))
                }
                ExtraBytesDataType::I8
                | ExtraBytesDataType::I16
                | ExtraBytesDataType::I32
                | ExtraBytesDataType::I64 => Ok(ExtraBytesColumn::Signed(ExtraBytesTypedColumn {
                    records,
                    record_field_offset: offset,
                    descriptor,
                    item: PhantomData::<i64>,
                })),
                ExtraBytesDataType::F32 | ExtraBytesDataType::F64 => {
                    Ok(ExtraBytesColumn::Float(ExtraBytesTypedColumn {
                        records,
                        record_field_offset: offset,
                        descriptor,
                        item: PhantomData::<f64>,
                    }))
                }
                ExtraBytesDataType::Undocumented
                | ExtraBytesDataType::Deprecated(_)
                | ExtraBytesDataType::Reserved(_) => {
                    Err(Error::NonNumericExtraBytesField(name.to_owned()))
                }
            }
        }
    }

    /// Returns a typed nullable column, mapping raw no-data values to `None`.
    pub fn nullable_column<'pointdata, 'vlr>(
        &'vlr self,
        name: &str,
        points: &'pointdata PointData,
    ) -> Result<ExtraBytesNullableColumn<'pointdata, 'vlr>> {
        self.ensure_point_data(points)?;
        let descriptor = self.descriptor_or_error(name)?;
        if !descriptor.data_type.is_scalar() {
            return Err(Error::NonNumericExtraBytesField(name.to_owned()));
        }
        let offset = points.record_len() - self.extra_bytes_len + descriptor.byte_offset;
        let records = points.raw_bytes().chunks_exact(points.record_len());
        if descriptor.options & (SCALE_BIT | OFFSET_BIT) != 0 {
            Ok(ExtraBytesNullableColumn::Float(
                ExtraBytesTypedNullableColumn {
                    records,
                    record_field_offset: offset,
                    descriptor,
                    item: PhantomData,
                },
            ))
        } else {
            match descriptor.data_type {
                ExtraBytesDataType::U8
                | ExtraBytesDataType::U16
                | ExtraBytesDataType::U32
                | ExtraBytesDataType::U64 => Ok(ExtraBytesNullableColumn::Unsigned(
                    ExtraBytesTypedNullableColumn {
                        records,
                        record_field_offset: offset,
                        descriptor,
                        item: PhantomData,
                    },
                )),
                ExtraBytesDataType::I8
                | ExtraBytesDataType::I16
                | ExtraBytesDataType::I32
                | ExtraBytesDataType::I64 => Ok(ExtraBytesNullableColumn::Signed(
                    ExtraBytesTypedNullableColumn {
                        records,
                        record_field_offset: offset,
                        descriptor,
                        item: PhantomData,
                    },
                )),
                ExtraBytesDataType::F32 | ExtraBytesDataType::F64 => Ok(
                    ExtraBytesNullableColumn::Float(ExtraBytesTypedNullableColumn {
                        records,
                        record_field_offset: offset,
                        descriptor,
                        item: PhantomData,
                    }),
                ),
                ExtraBytesDataType::Undocumented
                | ExtraBytesDataType::Deprecated(_)
                | ExtraBytesDataType::Reserved(_) => {
                    Err(Error::NonNumericExtraBytesField(name.to_owned()))
                }
            }
        }
    }

    /// Returns the raw bytes for a named descriptor.
    pub fn raw_column<'a>(
        &self,
        name: &str,
        points: &'a PointData,
    ) -> Result<ExtraBytesRawColumn<'a>> {
        self.ensure_point_data(points)?;
        let descriptor = self.descriptor_or_error(name)?;
        let offset = points.record_len() - self.extra_bytes_len + descriptor.byte_offset;
        Ok(ExtraBytesRawColumn {
            records: points.raw_bytes().chunks_exact(points.record_len()),
            record_field_offset: offset,
            byte_size: descriptor.byte_size,
        })
    }

    /// Returns undescribed trailing bytes for every point.
    pub fn undocumented_column<'a>(
        &self,
        points: &'a PointData,
    ) -> Result<Option<ExtraBytesRawColumn<'a>>> {
        self.ensure_point_data(points)?;
        let size = self.undocumented_bytes_len();
        if size == 0 {
            return Ok(None);
        }
        let offset = points.record_len() - self.extra_bytes_len + self.described_bytes_len;
        Ok(Some(ExtraBytesRawColumn {
            records: points.raw_bytes().chunks_exact(points.record_len()),
            record_field_offset: offset,
            byte_size: size,
        }))
    }

    /// Returns numeric Extra Bytes values from an owned point in descriptor order.
    pub fn values(&self, point: &Point) -> Result<impl Iterator<Item = Result<ExtraBytesValue>>> {
        self.ensure_point(point)?;
        Ok(self.descriptors.iter().map(|descriptor| {
            let range = descriptor.byte_offset..descriptor.byte_offset + descriptor.byte_size;
            descriptor
                .decode(&point.extra_bytes[range])
                .ok_or_else(|| Error::NonNumericExtraBytesField(descriptor.name.clone()))
        }))
    }

    /// Returns one numeric Extra Bytes value from an owned point.
    pub fn value_for_named_field(&self, name: &str, point: &Point) -> Result<ExtraBytesValue> {
        self.ensure_point(point)?;
        let descriptor = self.descriptor_or_error(name)?;
        let range = descriptor.byte_offset..descriptor.byte_offset + descriptor.byte_size;
        descriptor
            .decode(&point.extra_bytes[range])
            .ok_or_else(|| Error::NonNumericExtraBytesField(name.to_owned()))
    }

    /// Returns the raw bytes for a named field from a borrowed point.
    ///
    /// The returned slice has the descriptor's on-disk width. It is not
    /// decoded, and scale and offset are not applied.
    pub fn raw_value_for_named_field<'a>(&self, name: &str, point: &'a Point) -> Result<&'a [u8]> {
        self.ensure_point(point)?;
        let descriptor = self.descriptor_or_error(name)?;
        let range = descriptor.byte_offset..descriptor.byte_offset + descriptor.byte_size;
        Ok(&point.extra_bytes[range])
    }

    fn ensure_point_data(&self, points: &PointData) -> Result<()> {
        let actual = usize::from(points.format().extra_bytes);
        if actual == self.extra_bytes_len {
            Ok(())
        } else {
            Err(Error::PointDataExtraBytesMismatch(
                self.extra_bytes_len,
                actual,
            ))
        }
    }

    fn ensure_point(&self, point: &Point) -> Result<()> {
        let actual = point.extra_bytes.len();
        if actual == self.extra_bytes_len {
            Ok(())
        } else {
            Err(Error::PointExtraBytesMismatch(self.extra_bytes_len, actual))
        }
    }

    fn descriptor_or_error(&self, name: &str) -> Result<&ExtraBytesDescriptor> {
        self.descriptor(name)
            .ok_or_else(|| Error::ExtraBytesFieldNotFound(name.to_owned()))
    }
}

/// A numeric Extra Bytes column whose variant determines every item's type.
#[derive(Clone, Debug)]
pub enum ExtraBytesColumn<'pointdata, 'vlr> {
    /// An iterator over unsigned integer values.
    Unsigned(ExtraBytesTypedColumn<'pointdata, 'vlr, u64>),
    /// An iterator over signed integer values.
    Signed(ExtraBytesTypedColumn<'pointdata, 'vlr, i64>),
    /// An iterator over floating-point values, including transformed values.
    Float(ExtraBytesTypedColumn<'pointdata, 'vlr, f64>),
}

impl ExtraBytesColumn<'_, '_> {
    /// Returns the number of values remaining in this column.
    pub fn len(&self) -> usize {
        match self {
            Self::Unsigned(values) => values.len(),
            Self::Signed(values) => values.len(),
            Self::Float(values) => values.len(),
        }
    }

    /// Returns true when this column contains no remaining values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An iterator over one concrete numeric Extra Bytes type.
#[derive(Clone, Debug)]
pub struct ExtraBytesTypedColumn<'pointdata, 'vlr, T> {
    records: ChunksExact<'pointdata, u8>,
    record_field_offset: usize,
    descriptor: &'vlr ExtraBytesDescriptor,
    item: PhantomData<T>,
}

impl Iterator for ExtraBytesTypedColumn<'_, '_, u64> {
    type Item = u64;
    fn next(&mut self) -> Option<Self::Item> {
        self.records.next().map(|record| {
            let bytes = &record
                [self.record_field_offset..self.record_field_offset + self.descriptor.byte_size];
            match decode_value(self.descriptor.data_type, bytes) {
                Some(ExtraBytesValue::Unsigned(value)) => value,
                _ => unreachable!("validated unsigned Extra Bytes column"),
            }
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}
impl ExactSizeIterator for ExtraBytesTypedColumn<'_, '_, u64> {}

impl Iterator for ExtraBytesTypedColumn<'_, '_, i64> {
    type Item = i64;
    fn next(&mut self) -> Option<Self::Item> {
        self.records.next().map(|record| {
            let bytes = &record
                [self.record_field_offset..self.record_field_offset + self.descriptor.byte_size];
            match decode_value(self.descriptor.data_type, bytes) {
                Some(ExtraBytesValue::Signed(value)) => value,
                _ => unreachable!("validated signed Extra Bytes column"),
            }
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}
impl ExactSizeIterator for ExtraBytesTypedColumn<'_, '_, i64> {}

impl Iterator for ExtraBytesTypedColumn<'_, '_, f64> {
    type Item = f64;
    fn next(&mut self) -> Option<Self::Item> {
        self.records.next().map(|record| {
            let bytes = &record
                [self.record_field_offset..self.record_field_offset + self.descriptor.byte_size];
            match self.descriptor.decode(bytes) {
                Some(ExtraBytesValue::Float(value)) => value,
                _ => unreachable!("validated floating-point Extra Bytes column"),
            }
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}
impl ExactSizeIterator for ExtraBytesTypedColumn<'_, '_, f64> {}

/// A nullable Extra Bytes column whose variant determines every item's type.
#[derive(Clone, Debug)]
pub enum ExtraBytesNullableColumn<'pointdata, 'vlr> {
    /// An iterator over optional unsigned integer values.
    Unsigned(ExtraBytesTypedNullableColumn<'pointdata, 'vlr, u64>),
    /// An iterator over optional signed integer values.
    Signed(ExtraBytesTypedNullableColumn<'pointdata, 'vlr, i64>),
    /// An iterator over optional floating-point or transformed values.
    Float(ExtraBytesTypedNullableColumn<'pointdata, 'vlr, f64>),
}

impl ExtraBytesNullableColumn<'_, '_> {
    /// Returns the number of values remaining in this column.
    pub fn len(&self) -> usize {
        match self {
            Self::Unsigned(values) => values.len(),
            Self::Signed(values) => values.len(),
            Self::Float(values) => values.len(),
        }
    }

    /// Returns true when this column contains no remaining values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An iterator over one concrete optional numeric Extra Bytes type.
#[derive(Clone, Debug)]
pub struct ExtraBytesTypedNullableColumn<'pointdata, 'vlr, T> {
    records: ChunksExact<'pointdata, u8>,
    record_field_offset: usize,
    descriptor: &'vlr ExtraBytesDescriptor,
    item: PhantomData<T>,
}

impl<T> ExtraBytesTypedNullableColumn<'_, '_, T> {
    fn next_value(&mut self) -> Option<Option<ExtraBytesValue>> {
        self.records.next().map(|record| {
            let bytes = &record
                [self.record_field_offset..self.record_field_offset + self.descriptor.byte_size];
            let raw_value = decode_value(self.descriptor.data_type, bytes)
                .expect("validated Extra Bytes column");
            if self.descriptor.is_no_data(raw_value) {
                None
            } else {
                Some(
                    self.descriptor
                        .decode(bytes)
                        .expect("validated Extra Bytes column"),
                )
            }
        })
    }
}

impl Iterator for ExtraBytesTypedNullableColumn<'_, '_, u64> {
    type Item = Option<u64>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_value().map(|value| {
            value.map(|value| match value {
                ExtraBytesValue::Unsigned(value) => value,
                _ => unreachable!("validated unsigned Extra Bytes column"),
            })
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}
impl ExactSizeIterator for ExtraBytesTypedNullableColumn<'_, '_, u64> {}

impl Iterator for ExtraBytesTypedNullableColumn<'_, '_, i64> {
    type Item = Option<i64>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_value().map(|value| {
            value.map(|value| match value {
                ExtraBytesValue::Signed(value) => value,
                _ => unreachable!("validated signed Extra Bytes column"),
            })
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}
impl ExactSizeIterator for ExtraBytesTypedNullableColumn<'_, '_, i64> {}

impl Iterator for ExtraBytesTypedNullableColumn<'_, '_, f64> {
    type Item = Option<f64>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_value().map(|value| {
            value.map(|value| match value {
                ExtraBytesValue::Float(value) => value,
                _ => unreachable!("validated floating-point Extra Bytes column"),
            })
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}
impl ExactSizeIterator for ExtraBytesTypedNullableColumn<'_, '_, f64> {}

/// A raw byte column borrowed from a byte slab.
#[derive(Clone, Debug)]
pub struct ExtraBytesRawColumn<'a> {
    records: ChunksExact<'a, u8>,
    record_field_offset: usize,
    byte_size: usize,
}
impl<'a> Iterator for ExtraBytesRawColumn<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<Self::Item> {
        self.records.next().map(|record| {
            &record[self.record_field_offset..self.record_field_offset + self.byte_size]
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}
impl ExactSizeIterator for ExtraBytesRawColumn<'_> {}

fn parse_descriptor(bytes: &[u8], byte_offset: usize) -> Result<ExtraBytesDescriptor> {
    if bytes.len() != ExtraBytesVlr::DESCRIPTOR_SIZE {
        return Err(Error::InvalidExtraBytesVlrLength(bytes.len()));
    }
    let data_type = ExtraBytesDataType::from_code(bytes[2]);
    let options = bytes[3];
    let byte_size = data_type
        .byte_size(options)
        .ok_or(Error::ReservedExtraBytesDataType(data_type.code()))?;
    Ok(ExtraBytesDescriptor {
        data_type,
        options,
        name: fixed_string(&bytes[4..36]),
        no_data: decode_metadata(
            data_type,
            options,
            NO_DATA_BIT,
            bytes[40..48].try_into().expect("descriptor field"),
        ),
        min: decode_metadata(
            data_type,
            options,
            MIN_BIT,
            bytes[64..72].try_into().expect("descriptor field"),
        ),
        max: decode_metadata(
            data_type,
            options,
            MAX_BIT,
            bytes[88..96].try_into().expect("descriptor field"),
        ),
        scale: f64::from_le_bytes(bytes[112..120].try_into().expect("descriptor field")),
        offset: f64::from_le_bytes(bytes[136..144].try_into().expect("descriptor field")),
        description: fixed_string(&bytes[160..192]),
        byte_offset,
        byte_size,
    })
}

fn decode_metadata(
    data_type: ExtraBytesDataType,
    options: u8,
    option_bit: u8,
    bytes: [u8; 8],
) -> Option<ExtraBytesValue> {
    if options & option_bit == 0 || !data_type.is_scalar() {
        return None;
    }
    Some(match data_type {
        ExtraBytesDataType::U8
        | ExtraBytesDataType::U16
        | ExtraBytesDataType::U32
        | ExtraBytesDataType::U64 => ExtraBytesValue::Unsigned(u64::from_le_bytes(bytes)),
        ExtraBytesDataType::I8
        | ExtraBytesDataType::I16
        | ExtraBytesDataType::I32
        | ExtraBytesDataType::I64 => ExtraBytesValue::Signed(i64::from_le_bytes(bytes)),
        ExtraBytesDataType::F32 | ExtraBytesDataType::F64 => {
            ExtraBytesValue::Float(f64::from_le_bytes(bytes))
        }
        ExtraBytesDataType::Undocumented
        | ExtraBytesDataType::Deprecated(_)
        | ExtraBytesDataType::Reserved(_) => {
            unreachable!("unsupported Extra Bytes type")
        }
    })
}

fn decode_value(data_type: ExtraBytesDataType, bytes: &[u8]) -> Option<ExtraBytesValue> {
    Some(match data_type {
        ExtraBytesDataType::U8 => {
            ExtraBytesValue::Unsigned(u64::from(u8::from_le_bytes(bytes.try_into().ok()?)))
        }
        ExtraBytesDataType::I8 => {
            ExtraBytesValue::Signed(i64::from(i8::from_le_bytes(bytes.try_into().ok()?)))
        }
        ExtraBytesDataType::U16 => {
            ExtraBytesValue::Unsigned(u64::from(u16::from_le_bytes(bytes.try_into().ok()?)))
        }
        ExtraBytesDataType::I16 => {
            ExtraBytesValue::Signed(i64::from(i16::from_le_bytes(bytes.try_into().ok()?)))
        }
        ExtraBytesDataType::U32 => {
            ExtraBytesValue::Unsigned(u64::from(u32::from_le_bytes(bytes.try_into().ok()?)))
        }
        ExtraBytesDataType::I32 => {
            ExtraBytesValue::Signed(i64::from(i32::from_le_bytes(bytes.try_into().ok()?)))
        }
        ExtraBytesDataType::U64 => {
            ExtraBytesValue::Unsigned(u64::from_le_bytes(bytes.try_into().ok()?))
        }
        ExtraBytesDataType::I64 => {
            ExtraBytesValue::Signed(i64::from_le_bytes(bytes.try_into().ok()?))
        }
        ExtraBytesDataType::F32 => {
            ExtraBytesValue::Float(f64::from(f32::from_le_bytes(bytes.try_into().ok()?)))
        }
        ExtraBytesDataType::F64 => {
            ExtraBytesValue::Float(f64::from_le_bytes(bytes.try_into().ok()?))
        }
        ExtraBytesDataType::Undocumented
        | ExtraBytesDataType::Deprecated(_)
        | ExtraBytesDataType::Reserved(_) => return None,
    })
}

fn fixed_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{point::Format, PointDataBuilder};

    fn descriptor_with_metadata(
        data_type: u8,
        options: u8,
        no_data: [u8; 8],
        min: [u8; 8],
        max: [u8; 8],
    ) -> ExtraBytesDescriptor {
        let mut bytes = [0; ExtraBytesVlr::DESCRIPTOR_SIZE];
        bytes[2] = data_type;
        bytes[3] = options;
        bytes[40..48].copy_from_slice(&no_data);
        bytes[64..72].copy_from_slice(&min);
        bytes[88..96].copy_from_slice(&max);
        parse_descriptor(&bytes, 0).expect("valid descriptor")
    }

    fn point_data_with_u16(values: &[u16]) -> PointData {
        let mut format = Format::new(0).expect("point format");
        format.extra_bytes = 2;
        let record_len = usize::from(format.len());
        let mut bytes = vec![0; record_len * values.len()];
        for (record, value) in bytes.chunks_exact_mut(record_len).zip(values) {
            record[record_len - 2..].copy_from_slice(&value.to_le_bytes());
        }
        PointDataBuilder::new()
            .with_format(format)
            .build_from_bytes(bytes)
            .expect("point data")
    }

    fn extra_bytes_with_descriptor(mut descriptor: ExtraBytesDescriptor) -> ExtraBytesVlr {
        descriptor.name = "value".to_owned();
        ExtraBytesVlr {
            descriptors: vec![descriptor],
            extra_bytes_len: 2,
            described_bytes_len: 2,
        }
    }

    #[test]
    fn unsigned_metadata_stays_upcast() {
        let descriptor = descriptor_with_metadata(
            ExtraBytesDataType::U8.code(),
            NO_DATA_BIT | MIN_BIT | MAX_BIT,
            255_u64.to_le_bytes(),
            1_u64.to_le_bytes(),
            250_u64.to_le_bytes(),
        );
        assert_eq!(descriptor.no_data(), Some(ExtraBytesValue::Unsigned(255)));
        assert_eq!(descriptor.min(), Some(ExtraBytesValue::Unsigned(1)));
        assert_eq!(descriptor.max(), Some(ExtraBytesValue::Unsigned(250)));
    }

    #[test]
    fn signed_metadata_stays_upcast() {
        let descriptor = descriptor_with_metadata(
            ExtraBytesDataType::I16.code(),
            NO_DATA_BIT | MIN_BIT | MAX_BIT,
            (-32_768_i64).to_le_bytes(),
            (-12_345_i64).to_le_bytes(),
            12_345_i64.to_le_bytes(),
        );
        assert_eq!(descriptor.no_data(), Some(ExtraBytesValue::Signed(-32_768)));
        assert_eq!(descriptor.min(), Some(ExtraBytesValue::Signed(-12_345)));
        assert_eq!(descriptor.max(), Some(ExtraBytesValue::Signed(12_345)));
    }

    #[test]
    fn f32_metadata_keeps_stored_f64_precision() {
        let no_data = 1.234_567_890_123_f64;
        let descriptor = descriptor_with_metadata(
            ExtraBytesDataType::F32.code(),
            NO_DATA_BIT,
            no_data.to_le_bytes(),
            [0; 8],
            [0; 8],
        );
        assert_eq!(descriptor.no_data(), Some(ExtraBytesValue::Float(no_data)));
        assert_eq!(descriptor.min(), None);
        assert_eq!(descriptor.max(), None);
    }

    #[test]
    fn point_values_are_widened_to_three_public_types() {
        assert_eq!(
            decode_value(ExtraBytesDataType::U16, &42_u16.to_le_bytes()),
            Some(ExtraBytesValue::Unsigned(42))
        );
        assert_eq!(
            decode_value(ExtraBytesDataType::I8, &(-12_i8).to_le_bytes()),
            Some(ExtraBytesValue::Signed(-12))
        );
        assert_eq!(
            decode_value(ExtraBytesDataType::F32, &1.25_f32.to_le_bytes()),
            Some(ExtraBytesValue::Float(1.25))
        );
    }

    #[test]
    fn scale_or_offset_makes_the_value_f64() {
        let mut descriptor = descriptor_with_metadata(
            ExtraBytesDataType::U16.code(),
            SCALE_BIT | OFFSET_BIT,
            [0; 8],
            [0; 8],
            [0; 8],
        );
        descriptor.scale = 0.5;
        descriptor.offset = 10.0;
        assert_eq!(
            descriptor.decode(&20_u16.to_le_bytes()),
            Some(ExtraBytesValue::Float(20.0))
        );
    }

    #[test]
    fn column_selects_one_primitive_iterator_type() {
        let descriptor =
            descriptor_with_metadata(ExtraBytesDataType::U16.code(), 0, [0; 8], [0; 8], [0; 8]);
        let extra_bytes = extra_bytes_with_descriptor(descriptor);
        let points = point_data_with_u16(&[12, 34]);

        match extra_bytes.column("value", &points).expect("column") {
            ExtraBytesColumn::Unsigned(values) => {
                assert_eq!(values.collect::<Vec<_>>(), vec![12_u64, 34]);
            }
            ExtraBytesColumn::Signed(_) | ExtraBytesColumn::Float(_) => {
                panic!("expected unsigned column")
            }
        }
    }

    #[test]
    fn transformed_integer_column_selects_float_iterator() {
        let mut descriptor = descriptor_with_metadata(
            ExtraBytesDataType::U16.code(),
            SCALE_BIT | OFFSET_BIT,
            [0; 8],
            [0; 8],
            [0; 8],
        );
        descriptor.scale = 0.5;
        descriptor.offset = 10.0;
        let extra_bytes = extra_bytes_with_descriptor(descriptor);
        let points = point_data_with_u16(&[20, 40]);

        match extra_bytes.column("value", &points).expect("column") {
            ExtraBytesColumn::Float(values) => {
                assert_eq!(values.collect::<Vec<_>>(), vec![20.0_f64, 30.0]);
            }
            ExtraBytesColumn::Unsigned(_) | ExtraBytesColumn::Signed(_) => {
                panic!("expected float column")
            }
        }
    }

    #[test]
    fn nullable_column_also_selects_one_primitive_iterator_type() {
        let descriptor = descriptor_with_metadata(
            ExtraBytesDataType::U16.code(),
            NO_DATA_BIT,
            0_u64.to_le_bytes(),
            [0; 8],
            [0; 8],
        );
        let extra_bytes = extra_bytes_with_descriptor(descriptor);
        let points = point_data_with_u16(&[0, 7]);

        match extra_bytes
            .nullable_column("value", &points)
            .expect("nullable column")
        {
            ExtraBytesNullableColumn::Unsigned(values) => {
                assert_eq!(values.collect::<Vec<_>>(), vec![None, Some(7_u64)]);
            }
            ExtraBytesNullableColumn::Signed(_) | ExtraBytesNullableColumn::Float(_) => {
                panic!("expected unsigned column")
            }
        }
    }

    #[test]
    fn raw_named_value_borrows_the_point_bytes() {
        let mut descriptor =
            descriptor_with_metadata(ExtraBytesDataType::U16.code(), 0, [0; 8], [0; 8], [0; 8]);
        descriptor.name = "temperature".to_owned();
        let extra_bytes = ExtraBytesVlr {
            descriptors: vec![descriptor],
            extra_bytes_len: 2,
            described_bytes_len: 2,
        };
        let point = Point {
            extra_bytes: vec![0x34, 0x12],
            ..Point::default()
        };

        let raw = extra_bytes
            .raw_value_for_named_field("temperature", &point)
            .expect("named raw value");
        assert_eq!(raw, &[0x34, 0x12]);
        assert_eq!(raw.as_ptr(), point.extra_bytes.as_ptr());
    }
}

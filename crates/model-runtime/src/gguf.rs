//! Bounded inspection of user-selected GGUF model files.

use std::{
    collections::HashSet,
    error::Error,
    fmt,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

const GGUF_MAGIC: [u8; 4] = *b"GGUF";
const SUPPORTED_ARCHITECTURE: &str = "muse-glimmer";
const MAX_HEADER_BYTES: u64 = 128 * 1024 * 1024;
const MAX_METADATA_ENTRIES: u64 = 4_096;
const MAX_TENSORS: u64 = 100_000;
const MAX_ARRAY_ELEMENTS: u64 = 2_000_000;
const MAX_ARRAY_DEPTH: u8 = 4;
const MAX_KEY_BYTES: u64 = 256;
const MAX_NAME_BYTES: u64 = 512;
const MAX_TENSOR_NAME_BYTES: u64 = 1_024;
const MAX_DIMENSIONS: u32 = 4;
const MAX_CONTEXT_TOKENS: u32 = 1_048_576;
const DEFAULT_ALIGNMENT: u32 = 32;

const TYPE_UINT8: u32 = 0;
const TYPE_INT8: u32 = 1;
const TYPE_UINT16: u32 = 2;
const TYPE_INT16: u32 = 3;
const TYPE_UINT32: u32 = 4;
const TYPE_INT32: u32 = 5;
const TYPE_FLOAT32: u32 = 6;
const TYPE_BOOL: u32 = 7;
const TYPE_STRING: u32 = 8;
const TYPE_ARRAY: u32 = 9;
const TYPE_UINT64: u32 = 10;
const TYPE_INT64: u32 = 11;
const TYPE_FLOAT64: u32 = 12;

/// Relevant, validated metadata for one supported GGUF model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GgufMetadata {
    version: u32,
    architecture: String,
    name: String,
    context_tokens: u32,
    tensor_count: u64,
    metadata_count: u64,
    file_size: u64,
    file_type: Option<u32>,
    quantization_version: Option<u32>,
}

impl GgufMetadata {
    /// Returns the GGUF container version.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Returns the validated model architecture.
    #[must_use]
    pub fn architecture(&self) -> &str {
        &self.architecture
    }

    /// Returns the bounded human-readable model name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the model-declared maximum context length.
    #[must_use]
    pub const fn context_tokens(&self) -> u32 {
        self.context_tokens
    }

    /// Returns the number of tensor descriptors.
    #[must_use]
    pub const fn tensor_count(&self) -> u64 {
        self.tensor_count
    }

    /// Returns the number of metadata entries.
    #[must_use]
    pub const fn metadata_count(&self) -> u64 {
        self.metadata_count
    }

    /// Returns the complete source file size.
    #[must_use]
    pub const fn file_size(&self) -> u64 {
        self.file_size
    }

    /// Returns the optional GGUF file-type code.
    #[must_use]
    pub const fn file_type(&self) -> Option<u32> {
        self.file_type
    }

    /// Returns the optional quantization-format version.
    #[must_use]
    pub const fn quantization_version(&self) -> Option<u32> {
        self.quantization_version
    }
}

/// Opens and validates the bounded header of one GGUF file.
///
/// # Errors
///
/// Returns a typed error when the file is malformed, incomplete, too complex,
/// unsupported or incompatible with the bundled Glimmer runtime.
pub fn inspect_gguf(path: impl AsRef<Path>) -> Result<GgufMetadata, GgufError> {
    let mut file = File::open(path).map_err(GgufError::Io)?;
    inspect_gguf_file(&mut file)
}

pub(crate) fn inspect_gguf_file(file: &mut File) -> Result<GgufMetadata, GgufError> {
    let file_size = file.metadata().map_err(GgufError::Io)?.len();
    if file_size < 24 {
        return Err(GgufError::Truncated);
    }
    file.seek(SeekFrom::Start(0)).map_err(GgufError::Io)?;
    let limit = file_size.min(MAX_HEADER_BYTES);
    let mut reader = HeaderReader { file, limit };
    if reader.read_array::<4>()? != GGUF_MAGIC {
        return Err(GgufError::InvalidMagic);
    }
    let version = reader.read_u32()?;
    if !matches!(version, 2 | 3) {
        return Err(GgufError::UnsupportedVersion(version));
    }
    let tensor_count = reader.read_u64()?;
    if tensor_count == 0 || tensor_count > MAX_TENSORS {
        return Err(GgufError::InvalidTensorCount(tensor_count));
    }
    let metadata_count = reader.read_u64()?;
    if metadata_count == 0 || metadata_count > MAX_METADATA_ENTRIES {
        return Err(GgufError::InvalidMetadataCount(metadata_count));
    }

    let mut architecture = None;
    let mut model_type = None;
    let mut name = None;
    let mut context_tokens = None;
    let mut alignment = None;
    let mut file_type = None;
    let mut quantization_version = None;
    let mut keys = HashSet::with_capacity(usize::try_from(metadata_count).unwrap_or(0));
    for _ in 0..metadata_count {
        let key = reader.read_string(MAX_KEY_BYTES)?;
        validate_key(&key)?;
        if !keys.insert(key.clone()) {
            return Err(GgufError::DuplicateMetadataKey(key));
        }
        let value_type = reader.read_u32()?;
        match key.as_str() {
            "general.architecture" => {
                architecture = Some(reader.read_typed_string(value_type, MAX_NAME_BYTES, &key)?);
            }
            "general.type" => {
                model_type = Some(reader.read_typed_string(value_type, MAX_NAME_BYTES, &key)?);
            }
            "general.name" => {
                name = Some(reader.read_typed_string(value_type, MAX_NAME_BYTES, &key)?);
            }
            "muse-glimmer.context_length" => {
                context_tokens = Some(reader.read_typed_u32(value_type, &key)?);
            }
            "general.alignment" => {
                alignment = Some(reader.read_typed_u32(value_type, &key)?);
            }
            "general.file_type" => {
                file_type = Some(reader.read_typed_u32(value_type, &key)?);
            }
            "general.quantization_version" => {
                quantization_version = Some(reader.read_typed_u32(value_type, &key)?);
            }
            _ => reader.skip_value(value_type, 0)?,
        }
    }

    let architecture = architecture.ok_or(GgufError::MissingMetadata("general.architecture"))?;
    if architecture != SUPPORTED_ARCHITECTURE {
        return Err(GgufError::UnsupportedArchitecture(architecture));
    }
    if model_type.as_deref().is_some_and(|value| value != "model") {
        return Err(GgufError::UnsupportedModelType(
            model_type.unwrap_or_default(),
        ));
    }
    let name = name.ok_or(GgufError::MissingMetadata("general.name"))?;
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(GgufError::InvalidMetadata("general.name"));
    }
    let context_tokens =
        context_tokens.ok_or(GgufError::MissingMetadata("muse-glimmer.context_length"))?;
    if context_tokens == 0 || context_tokens > MAX_CONTEXT_TOKENS {
        return Err(GgufError::UnsupportedContext(context_tokens));
    }
    let alignment = alignment.unwrap_or(DEFAULT_ALIGNMENT);
    if alignment < 8 || !alignment.is_power_of_two() {
        return Err(GgufError::InvalidAlignment(alignment));
    }

    validate_tensor_descriptors(&mut reader, tensor_count, alignment, file_size)?;
    Ok(GgufMetadata {
        version,
        architecture,
        name,
        context_tokens,
        tensor_count,
        metadata_count,
        file_size,
        file_type,
        quantization_version,
    })
}

fn validate_tensor_descriptors(
    reader: &mut HeaderReader<'_>,
    tensor_count: u64,
    alignment: u32,
    file_size: u64,
) -> Result<(), GgufError> {
    let mut names = HashSet::with_capacity(usize::try_from(tensor_count).unwrap_or(0));
    let mut offsets = Vec::with_capacity(usize::try_from(tensor_count).unwrap_or(0));
    for _ in 0..tensor_count {
        let name = reader.read_string(MAX_TENSOR_NAME_BYTES)?;
        if name.is_empty() || name.chars().any(char::is_control) || !names.insert(name.clone()) {
            return Err(GgufError::InvalidTensorDescriptor(name));
        }
        let dimensions = reader.read_u32()?;
        if dimensions == 0 || dimensions > MAX_DIMENSIONS {
            return Err(GgufError::InvalidDimensions(dimensions));
        }
        let mut elements = 1_u64;
        for _ in 0..dimensions {
            let dimension = reader.read_u64()?;
            if dimension == 0 {
                return Err(GgufError::InvalidDimensions(dimensions));
            }
            elements = elements
                .checked_mul(dimension)
                .ok_or(GgufError::InvalidTensorShape)?;
        }
        let _tensor_type = reader.read_u32()?;
        let offset = reader.read_u64()?;
        if offset % u64::from(alignment) != 0 {
            return Err(GgufError::MisalignedTensor(offset));
        }
        offsets.push(offset);
    }
    let descriptor_end = reader.position()?;
    let data_start = align_up(descriptor_end, u64::from(alignment))?;
    if data_start >= file_size {
        return Err(GgufError::Truncated);
    }
    let data_bytes = file_size - data_start;
    if offsets.into_iter().any(|offset| offset >= data_bytes) {
        return Err(GgufError::TensorOutsideFile);
    }
    Ok(())
}

fn align_up(value: u64, alignment: u64) -> Result<u64, GgufError> {
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or(GgufError::InvalidTensorShape)
}

fn validate_key(key: &str) -> Result<(), GgufError> {
    if key.is_empty()
        || !key.is_ascii()
        || key.starts_with('.')
        || key.ends_with('.')
        || key.split('.').any(|segment| {
            segment.is_empty()
                || !segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-')
                })
        })
    {
        return Err(GgufError::InvalidMetadataKey(key.to_owned()));
    }
    Ok(())
}

struct HeaderReader<'a> {
    file: &'a mut File,
    limit: u64,
}

impl HeaderReader<'_> {
    fn position(&mut self) -> Result<u64, GgufError> {
        self.file.stream_position().map_err(GgufError::Io)
    }

    fn ensure(&mut self, bytes: u64) -> Result<(), GgufError> {
        let end = self
            .position()?
            .checked_add(bytes)
            .ok_or(GgufError::HeaderTooLarge)?;
        if end > self.limit {
            return Err(if self.limit == MAX_HEADER_BYTES {
                GgufError::HeaderTooLarge
            } else {
                GgufError::Truncated
            });
        }
        Ok(())
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], GgufError> {
        self.ensure(u64::try_from(N).map_err(|_| GgufError::HeaderTooLarge)?)?;
        let mut bytes = [0_u8; N];
        self.file.read_exact(&mut bytes).map_err(map_read_error)?;
        Ok(bytes)
    }

    fn read_u32(&mut self) -> Result<u32, GgufError> {
        self.read_array::<4>().map(u32::from_le_bytes)
    }

    fn read_u64(&mut self) -> Result<u64, GgufError> {
        self.read_array::<8>().map(u64::from_le_bytes)
    }

    fn read_string(&mut self, max_bytes: u64) -> Result<String, GgufError> {
        let length = self.read_u64()?;
        if length > max_bytes {
            return Err(GgufError::StringTooLarge(length));
        }
        self.ensure(length)?;
        let length = usize::try_from(length).map_err(|_| GgufError::StringTooLarge(length))?;
        let mut bytes = vec![0_u8; length];
        self.file.read_exact(&mut bytes).map_err(map_read_error)?;
        String::from_utf8(bytes).map_err(|_| GgufError::InvalidUtf8)
    }

    fn read_typed_string(
        &mut self,
        value_type: u32,
        max_bytes: u64,
        key: &str,
    ) -> Result<String, GgufError> {
        if value_type != TYPE_STRING {
            return Err(GgufError::MetadataTypeMismatch(key.to_owned()));
        }
        self.read_string(max_bytes)
    }

    fn read_typed_u32(&mut self, value_type: u32, key: &str) -> Result<u32, GgufError> {
        if value_type != TYPE_UINT32 {
            return Err(GgufError::MetadataTypeMismatch(key.to_owned()));
        }
        self.read_u32()
    }

    fn skip_value(&mut self, value_type: u32, depth: u8) -> Result<(), GgufError> {
        match value_type {
            TYPE_UINT8 | TYPE_INT8 | TYPE_BOOL => self.skip(1),
            TYPE_UINT16 | TYPE_INT16 => self.skip(2),
            TYPE_UINT32 | TYPE_INT32 | TYPE_FLOAT32 => self.skip(4),
            TYPE_UINT64 | TYPE_INT64 | TYPE_FLOAT64 => self.skip(8),
            TYPE_STRING => {
                let length = self.read_u64()?;
                self.skip(length)
            }
            TYPE_ARRAY => {
                if depth >= MAX_ARRAY_DEPTH {
                    return Err(GgufError::ArrayTooDeep);
                }
                let element_type = self.read_u32()?;
                let length = self.read_u64()?;
                if length > MAX_ARRAY_ELEMENTS {
                    return Err(GgufError::ArrayTooLarge(length));
                }
                if let Some(width) = primitive_width(element_type) {
                    let bytes = length.checked_mul(width).ok_or(GgufError::HeaderTooLarge)?;
                    self.skip(bytes)
                } else {
                    for _ in 0..length {
                        self.skip_value(element_type, depth + 1)?;
                    }
                    Ok(())
                }
            }
            other => Err(GgufError::UnknownValueType(other)),
        }
    }

    fn skip(&mut self, bytes: u64) -> Result<(), GgufError> {
        self.ensure(bytes)?;
        let offset = i64::try_from(bytes).map_err(|_| GgufError::HeaderTooLarge)?;
        self.file
            .seek(SeekFrom::Current(offset))
            .map(|_| ())
            .map_err(GgufError::Io)
    }
}

const fn primitive_width(value_type: u32) -> Option<u64> {
    match value_type {
        TYPE_UINT8 | TYPE_INT8 | TYPE_BOOL => Some(1),
        TYPE_UINT16 | TYPE_INT16 => Some(2),
        TYPE_UINT32 | TYPE_INT32 | TYPE_FLOAT32 => Some(4),
        TYPE_UINT64 | TYPE_INT64 | TYPE_FLOAT64 => Some(8),
        _ => None,
    }
}

fn map_read_error(error: io::Error) -> GgufError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        GgufError::Truncated
    } else {
        GgufError::Io(error)
    }
}

/// Failure while inspecting a GGUF model file.
#[derive(Debug)]
pub enum GgufError {
    /// The file could not be read.
    Io(io::Error),
    /// The file is too short for its declared structure.
    Truncated,
    /// The GGUF magic bytes are absent.
    InvalidMagic,
    /// This GGUF container version is not supported.
    UnsupportedVersion(u32),
    /// The metadata entry count is zero or exceeds the inspection limit.
    InvalidMetadataCount(u64),
    /// The tensor count is zero or exceeds the inspection limit.
    InvalidTensorCount(u64),
    /// The metadata and tensor-information header exceeds its byte limit.
    HeaderTooLarge,
    /// A GGUF string exceeds its field limit.
    StringTooLarge(u64),
    /// A GGUF string is not valid UTF-8.
    InvalidUtf8,
    /// A metadata key is malformed.
    InvalidMetadataKey(String),
    /// A metadata key occurs more than once.
    DuplicateMetadataKey(String),
    /// A metadata value uses an unknown GGUF type.
    UnknownValueType(u32),
    /// An array exceeds the element limit.
    ArrayTooLarge(u64),
    /// Nested metadata arrays exceed the depth limit.
    ArrayTooDeep,
    /// A required field is absent.
    MissingMetadata(&'static str),
    /// A relevant field has the wrong GGUF value type.
    MetadataTypeMismatch(String),
    /// A relevant field contains an invalid value.
    InvalidMetadata(&'static str),
    /// The architecture cannot be run by the Glimmer profile.
    UnsupportedArchitecture(String),
    /// The GGUF is an adapter, projector or other non-model artifact.
    UnsupportedModelType(String),
    /// The context length is absent from the supported range.
    UnsupportedContext(u32),
    /// The tensor alignment is invalid.
    InvalidAlignment(u32),
    /// A tensor descriptor has an invalid or duplicate name.
    InvalidTensorDescriptor(String),
    /// A tensor has an unsupported dimension count.
    InvalidDimensions(u32),
    /// Tensor dimensions overflow the supported numeric range.
    InvalidTensorShape,
    /// A tensor offset is not aligned as declared.
    MisalignedTensor(u64),
    /// A tensor offset points beyond the end of the file.
    TensorOutsideFile,
}

impl fmt::Display for GgufError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => formatter.write_str("the GGUF file could not be read"),
            Self::Truncated => formatter.write_str("the GGUF file is truncated"),
            Self::InvalidMagic => formatter.write_str("the selected file is not a GGUF model"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "GGUF version {version} is not supported")
            }
            Self::InvalidMetadataCount(count) => {
                write!(formatter, "GGUF metadata count {count} is invalid")
            }
            Self::InvalidTensorCount(count) => {
                write!(formatter, "GGUF tensor count {count} is invalid")
            }
            Self::HeaderTooLarge => formatter.write_str("the GGUF header exceeds its safety limit"),
            Self::StringTooLarge(length) => {
                write!(
                    formatter,
                    "a GGUF string exceeds its safety limit ({length} bytes)"
                )
            }
            Self::InvalidUtf8 => formatter.write_str("GGUF metadata contains invalid UTF-8"),
            Self::InvalidMetadataKey(key) => write!(formatter, "invalid GGUF metadata key {key}"),
            Self::DuplicateMetadataKey(key) => {
                write!(formatter, "duplicate GGUF metadata key {key}")
            }
            Self::UnknownValueType(value_type) => {
                write!(formatter, "unknown GGUF metadata value type {value_type}")
            }
            Self::ArrayTooLarge(length) => {
                write!(
                    formatter,
                    "a GGUF metadata array is too large ({length} elements)"
                )
            }
            Self::ArrayTooDeep => formatter.write_str("GGUF metadata arrays are nested too deeply"),
            Self::MissingMetadata(key) => write!(formatter, "GGUF metadata is missing {key}"),
            Self::MetadataTypeMismatch(key) => {
                write!(formatter, "GGUF metadata {key} has the wrong value type")
            }
            Self::InvalidMetadata(key) => write!(formatter, "GGUF metadata {key} is invalid"),
            Self::UnsupportedArchitecture(architecture) => write!(
                formatter,
                "GGUF architecture {architecture} is unsupported; expected {SUPPORTED_ARCHITECTURE}"
            ),
            Self::UnsupportedModelType(model_type) => {
                write!(
                    formatter,
                    "GGUF artifact type {model_type} is not a standalone model"
                )
            }
            Self::UnsupportedContext(context) => {
                write!(formatter, "GGUF context length {context} is unsupported")
            }
            Self::InvalidAlignment(alignment) => {
                write!(formatter, "GGUF alignment {alignment} is invalid")
            }
            Self::InvalidTensorDescriptor(name) => {
                write!(formatter, "GGUF tensor descriptor {name} is invalid")
            }
            Self::InvalidDimensions(dimensions) => {
                write!(
                    formatter,
                    "GGUF tensor dimension count {dimensions} is invalid"
                )
            }
            Self::InvalidTensorShape => formatter.write_str("a GGUF tensor shape is invalid"),
            Self::MisalignedTensor(offset) => {
                write!(formatter, "GGUF tensor offset {offset} is misaligned")
            }
            Self::TensorOutsideFile => {
                formatter.write_str("a GGUF tensor points outside the selected file")
            }
        }
    }
}

impl Error for GgufError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs};

    use tempfile::TempDir;

    use super::*;

    fn append_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), io::Error> {
        let length = u64::try_from(value.len()).map_err(io::Error::other)?;
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn append_string_metadata(
        bytes: &mut Vec<u8>,
        key: &str,
        value: &str,
    ) -> Result<(), io::Error> {
        append_string(bytes, key)?;
        bytes.extend_from_slice(&TYPE_STRING.to_le_bytes());
        append_string(bytes, value)
    }

    fn append_u32_metadata(bytes: &mut Vec<u8>, key: &str, value: u32) -> Result<(), io::Error> {
        append_string(bytes, key)?;
        bytes.extend_from_slice(&TYPE_UINT32.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn valid_gguf(architecture: &str) -> Result<Vec<u8>, io::Error> {
        valid_gguf_with_offset(architecture, 0)
    }

    fn valid_gguf_with_offset(
        architecture: &str,
        tensor_offset: u64,
    ) -> Result<Vec<u8>, io::Error> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&GGUF_MAGIC);
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&5_u64.to_le_bytes());
        append_string_metadata(&mut bytes, "general.architecture", architecture)?;
        append_string_metadata(&mut bytes, "general.type", "model")?;
        append_string_metadata(&mut bytes, "general.name", "Test Glimmer")?;
        append_u32_metadata(&mut bytes, "muse-glimmer.context_length", 131_072)?;
        append_u32_metadata(&mut bytes, "general.alignment", 32)?;
        append_string(&mut bytes, "token_embd.weight")?;
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&tensor_offset.to_le_bytes());
        while bytes.len() % 32 != 0 {
            bytes.push(0);
        }
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        Ok(bytes)
    }

    #[test]
    fn inspects_supported_glimmer_metadata() -> Result<(), Box<dyn Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("model.gguf");
        fs::write(&path, valid_gguf(SUPPORTED_ARCHITECTURE)?)?;
        let metadata = inspect_gguf(path)?;
        assert_eq!(metadata.version(), 3);
        assert_eq!(metadata.architecture(), SUPPORTED_ARCHITECTURE);
        assert_eq!(metadata.name(), "Test Glimmer");
        assert_eq!(metadata.context_tokens(), 131_072);
        assert_eq!(metadata.tensor_count(), 1);
        assert_eq!(metadata.metadata_count(), 5);
        Ok(())
    }

    #[test]
    fn rejects_non_gguf_truncated_and_unsupported_architecture() -> Result<(), Box<dyn Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("model.gguf");
        fs::write(&path, b"not a model")?;
        assert!(matches!(inspect_gguf(&path), Err(GgufError::Truncated)));
        let mut invalid_magic = valid_gguf(SUPPORTED_ARCHITECTURE)?;
        invalid_magic[0] = b'X';
        fs::write(&path, invalid_magic)?;
        assert!(matches!(inspect_gguf(&path), Err(GgufError::InvalidMagic)));
        fs::write(&path, valid_gguf("llama")?)?;
        assert!(matches!(
            inspect_gguf(path),
            Err(GgufError::UnsupportedArchitecture(architecture)) if architecture == "llama"
        ));
        Ok(())
    }

    #[test]
    fn rejects_header_counts_and_tensor_offsets_outside_file() -> Result<(), Box<dyn Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("model.gguf");
        let mut too_many = valid_gguf(SUPPORTED_ARCHITECTURE)?;
        too_many[16..24].copy_from_slice(&(MAX_METADATA_ENTRIES + 1).to_le_bytes());
        fs::write(&path, too_many)?;
        assert!(matches!(
            inspect_gguf(&path),
            Err(GgufError::InvalidMetadataCount(_))
        ));

        fs::write(&path, valid_gguf_with_offset(SUPPORTED_ARCHITECTURE, 32)?)?;
        assert!(matches!(
            inspect_gguf(path),
            Err(GgufError::TensorOutsideFile)
        ));
        Ok(())
    }
}

//! Resource preflight only. Alloy decodes values; exact re-encoding below its
//! caller rejects aliases, overlap, padding, trailing bytes and other ambiguity.
use super::super::ContractError;
use alloy_dyn_abi::DynSolType;

pub(super) fn guard(types: &[DynSolType], data: &[u8]) -> Result<(), ContractError> {
    if data.len() > super::MAX_BYTES {
        return Err(ContractError::MalformedData);
    }
    Layout { data, nodes: 0 }.sequence(types.iter(), 0)
}
struct Layout<'a> {
    data: &'a [u8],
    nodes: usize,
}
impl Layout<'_> {
    fn range(&self, start: usize, size: usize) -> Result<(), ContractError> {
        self.data
            .get(
                start
                    ..start
                        .checked_add(size)
                        .ok_or(ContractError::MalformedData)?,
            )
            .ok_or(ContractError::MalformedData)
            .map(|_| ())
    }
    fn length(&self, start: usize) -> Result<usize, ContractError> {
        self.range(start, 32)?;
        let word = &self.data[start..start + 32];
        // Every admitted offset/length is <=8192; never narrow an arbitrary U256.
        if word[..30].iter().any(|byte| *byte != 0) {
            return Err(ContractError::MalformedData);
        }
        Ok(usize::from(u16::from_be_bytes([word[30], word[31]])))
    }
    fn sequence<'a>(
        &mut self,
        types: impl Iterator<Item = &'a DynSolType> + Clone,
        base: usize,
    ) -> Result<(), ContractError> {
        let head = types.clone().try_fold(0usize, |total, ty| {
            total
                .checked_add(head_size(ty))
                .ok_or(ContractError::MalformedData)
        })?;
        self.range(base, head)?;
        let mut cursor = base;
        for ty in types {
            let actual = if ty.is_dynamic() {
                let offset = self.length(cursor)?;
                if offset < head || offset % 32 != 0 {
                    return Err(ContractError::MalformedData);
                }
                base.checked_add(offset)
                    .ok_or(ContractError::MalformedData)?
            } else {
                cursor
            };
            self.value(ty, actual)?;
            cursor += head_size(ty);
        }
        Ok(())
    }
    fn value(&mut self, ty: &DynSolType, start: usize) -> Result<(), ContractError> {
        self.nodes += 1;
        if self.nodes > super::MAX_NODES {
            return Err(ContractError::MalformedData);
        }
        match ty {
            DynSolType::Bytes | DynSolType::String => {
                let length = self.length(start)?;
                if length > super::MAX_BLOB {
                    return Err(ContractError::MalformedData);
                }
                self.range(start + 32, length.div_ceil(32) * 32)
            }
            DynSolType::Array(inner) => {
                let length = self.length(start)?;
                if length > super::MAX_ITEMS {
                    return Err(ContractError::MalformedData);
                }
                self.sequence(std::iter::repeat_n(inner.as_ref(), length), start + 32)
            }
            DynSolType::FixedArray(inner, length) => {
                self.sequence(std::iter::repeat_n(inner.as_ref(), *length), start)
            }
            DynSolType::Tuple(fields) => self.sequence(fields.iter(), start),
            _ => self.scalar(ty, start),
        }
    }

    // Alloy's dynamic integer detokeniser deliberately reads a full 256-bit
    // word with a width label. Re-encoding alone therefore cannot establish
    // narrow integer range/sign extension; enforce those wire constraints here.
    fn scalar(&self, ty: &DynSolType, start: usize) -> Result<(), ContractError> {
        self.range(start, 32)?;
        let word = &self.data[start..start + 32];
        let valid = match ty {
            DynSolType::Uint(bits) => word[..32 - bits / 8].iter().all(|b| *b == 0),
            DynSolType::Int(bits) => {
                let high = 32 - bits / 8;
                let padding = if word[high] & 0x80 == 0 { 0 } else { 0xff };
                word[..high].iter().all(|b| *b == padding)
            }
            DynSolType::Address => word[..12].iter().all(|b| *b == 0),
            DynSolType::Bool => word[..31].iter().all(|b| *b == 0) && word[31] <= 1,
            DynSolType::FixedBytes(size) => word[*size..].iter().all(|b| *b == 0),
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(ContractError::MalformedData)
        }
    }
}
fn head_size(ty: &DynSolType) -> usize {
    if ty.is_dynamic() {
        return 32;
    }
    match ty {
        DynSolType::FixedArray(inner, size) => head_size(inner) * size,
        DynSolType::Tuple(fields) => fields.iter().map(head_size).sum(),
        _ => 32,
    }
}

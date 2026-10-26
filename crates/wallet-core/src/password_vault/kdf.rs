use super::{PasswordVaultError, WalletPassword};
use argon2::{Algorithm, Argon2, Block, Params, Version};
use std::sync::Mutex;
use zeroize::Zeroizing;

pub(crate) const MEMORY_KIB: u32 = 262_144;
pub(crate) const ITERATIONS: u32 = 6;
pub(crate) const LANES: u32 = 1;
static KDF_ADMISSION: Mutex<()> = Mutex::new(());

pub(crate) fn derive(
    password: &WalletPassword,
    salt: &[u8; 32],
) -> Result<Zeroizing<[u8; 32]>, PasswordVaultError> {
    derive_parameters(password.bytes(), salt, MEMORY_KIB, ITERATIONS, LANES)
}

fn derive_parameters(
    password: &[u8],
    salt: &[u8],
    memory: u32,
    iterations: u32,
    lanes: u32,
) -> Result<Zeroizing<[u8; 32]>, PasswordVaultError> {
    // Serialise scratch allocation even across distinct wallet coordinators.
    let _admission = KDF_ADMISSION
        .lock()
        .map_err(|_| PasswordVaultError::CryptographyUnavailable)?;
    let params = Params::new(memory, iterations, lanes, Some(32))
        .map_err(|_| PasswordVaultError::CryptographyUnavailable)?;
    let mut scratch = Zeroizing::new(Vec::new());
    scratch
        .try_reserve_exact(params.block_count())
        .map_err(|_| PasswordVaultError::CryptographyUnavailable)?;
    scratch.resize(params.block_count(), Block::new());
    let mut output = Zeroizing::new([0_u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into_with_memory(password, salt, output.as_mut(), scratch.as_mut_slice())
        .map_err(|_| PasswordVaultError::CryptographyUnavailable)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc9106_argon2id_known_answer() -> Result<(), Box<dyn std::error::Error>> {
        let params = argon2::ParamsBuilder::new()
            .m_cost(32)
            .t_cost(3)
            .p_cost(4)
            .data(argon2::AssociatedData::new(&[4; 12]).map_err(|_| "parameters")?)
            .build()
            .map_err(|_| "parameters")?;
        let argon = Argon2::new_with_secret(&[3; 8], Algorithm::Argon2id, Version::V0x13, params)
            .map_err(|_| "parameters")?;
        let mut scratch = Zeroizing::new(vec![Block::new(); 32]);
        let mut out = Zeroizing::new([0_u8; 32]);
        argon
            .hash_password_into_with_memory(
                &[1; 32],
                &[2; 16],
                out.as_mut(),
                scratch.as_mut_slice(),
            )
            .map_err(|_| "kdf")?;
        assert_eq!(
            *out,
            [
                0x0d, 0x64, 0x0d, 0xf5, 0x8d, 0x78, 0x76, 0x6c, 0x08, 0xc0, 0x37, 0xa3, 0x4a, 0x8b,
                0x53, 0xc9, 0xd0, 0x1e, 0xf0, 0x45, 0x2d, 0x75, 0xb6, 0x5e, 0xb5, 0x25, 0x20, 0xe9,
                0x6b, 0x01, 0xe6, 0x59
            ]
        );
        Ok(())
    }
    #[test]
    #[ignore = "Explicit KDF profile qualification benchmark; uses synthetic bytes only"]
    fn benchmark_profile_candidates() -> Result<(), PasswordVaultError> {
        for (memory, iterations) in [(65_536, 3), (131_072, 3), (262_144, 3), (262_144, 6)] {
            let start = std::time::Instant::now();
            let _derived = derive_parameters(
                b"synthetic benchmark password",
                &[0x42; 32],
                memory,
                iterations,
                1,
            )?;
            println!(
                "argon2id memory_kib={memory} iterations={iterations} lanes=1 elapsed_ms={}",
                start.elapsed().as_millis()
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "Explicit selected-profile latency/RSS qualification; synthetic bytes only"]
    fn qualify_selected_profile() -> Result<(), PasswordVaultError> {
        let password = WalletPassword::new("synthetic selected profile passphrase".to_owned())?;
        for run in 1..=5 {
            let start = std::time::Instant::now();
            let _derived = derive(&password, &[0x42; 32])?;
            println!(
                "selected_argon2id run={run} memory_kib={MEMORY_KIB} iterations={ITERATIONS} lanes={LANES} elapsed_ms={}",
                start.elapsed().as_millis()
            );
        }
        Ok(())
    }
}

//! Minimal PE header check (pure): is this file a DLL for the helper's own architecture?
//! Gives a clear error instead of LoadLibrary's ERROR_BAD_EXE_FORMAT when the x86 DLL is handed
//! to the x64 helper or vice versa.

pub const MACHINE_I386: u16 = 0x014c;
pub const MACHINE_AMD64: u16 = 0x8664;
pub const MACHINE_ARM64: u16 = 0xaa64;
const IMAGE_FILE_DLL: u16 = 0x2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeError {
    NotPe,
    NotDll,
    WrongMachine { found: u16, expected: u16 },
}

/// Machine of the running helper (x64 helper registers the 64-bit DLL, x86 the 32-bit one).
pub const fn own_machine() -> u16 {
    if cfg!(target_arch = "x86_64") {
        MACHINE_AMD64
    } else if cfg!(target_arch = "x86") {
        MACHINE_I386
    } else {
        MACHINE_ARM64
    }
}

pub fn machine_name(m: u16) -> &'static str {
    match m {
        MACHINE_I386 => "x86",
        MACHINE_AMD64 => "x64",
        MACHINE_ARM64 => "arm64",
        _ => "unknown",
    }
}

/// Reads machine and characteristics from the first bytes of a PE file (4 KiB is plenty).
pub fn inspect(head: &[u8]) -> Result<(u16, u16), PeError> {
    let u16_at = |o: usize| head.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| {
        head.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    if head.get(0..2) != Some(b"MZ") {
        return Err(PeError::NotPe);
    }
    let pe = u32_at(0x3c).ok_or(PeError::NotPe)? as usize;
    if head.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err(PeError::NotPe);
    }
    let machine = u16_at(pe + 4).ok_or(PeError::NotPe)?;
    let characteristics = u16_at(pe + 22).ok_or(PeError::NotPe)?;
    Ok((machine, characteristics))
}

/// Ok if `head` is a DLL for `expected`.
pub fn check_dll(head: &[u8], expected: u16) -> Result<(), PeError> {
    let (machine, characteristics) = inspect(head)?;
    if characteristics & IMAGE_FILE_DLL == 0 {
        return Err(PeError::NotDll);
    }
    if machine != expected {
        return Err(PeError::WrongMachine {
            found: machine,
            expected,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(machine: u16, characteristics: u16) -> Vec<u8> {
        let mut v = vec![0u8; 0x200];
        v[0..2].copy_from_slice(b"MZ");
        v[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        v[0x80..0x84].copy_from_slice(b"PE\0\0");
        v[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        v[0x96..0x98].copy_from_slice(&characteristics.to_le_bytes());
        v
    }

    #[test]
    fn accepts_matching_dll() {
        assert_eq!(
            check_dll(&image(MACHINE_AMD64, 0x2022), MACHINE_AMD64),
            Ok(())
        );
        assert_eq!(
            check_dll(&image(MACHINE_I386, 0x2102), MACHINE_I386),
            Ok(())
        );
    }

    #[test]
    fn rejects() {
        assert_eq!(
            check_dll(&image(MACHINE_I386, 0x2102), MACHINE_AMD64),
            Err(PeError::WrongMachine {
                found: MACHINE_I386,
                expected: MACHINE_AMD64
            })
        );
        assert_eq!(
            check_dll(&image(MACHINE_AMD64, 0x0022), MACHINE_AMD64),
            Err(PeError::NotDll)
        );
        assert_eq!(check_dll(b"MZ", MACHINE_AMD64), Err(PeError::NotPe));
        assert_eq!(check_dll(b"", MACHINE_AMD64), Err(PeError::NotPe));
        let mut bad = image(MACHINE_AMD64, 0x2022);
        bad[0x3c..0x40].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert_eq!(check_dll(&bad, MACHINE_AMD64), Err(PeError::NotPe));
    }

    #[test]
    fn own_machine_matches_build() {
        #[cfg(target_arch = "x86_64")]
        assert_eq!(own_machine(), MACHINE_AMD64);
        #[cfg(target_arch = "x86")]
        assert_eq!(own_machine(), MACHINE_I386);
        assert_ne!(machine_name(own_machine()), "unknown");
    }
}

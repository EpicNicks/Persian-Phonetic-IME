//! Spike-only GUIDs. Deliberately different from the real TIP's (frozen later in
//! `pime-tip/src/guids.rs`) so a leftover spike registration can never shadow the product.
use windows_core::GUID;

pub const CLSID_TEXT_SERVICE: GUID = GUID::from_u128(0x674850a2_4c3b_46fa_9b17_3ca5465ffa18);
pub const GUID_PROFILE: GUID = GUID::from_u128(0xda9a94b1_be61_4ff6_a706_7feaf1e62a33);
pub const GUID_DISPLAY_ATTR_INPUT: GUID = GUID::from_u128(0x802ad415_64ab_4cc4_832e_06a2666f27bc);

pub const LANGID_FA_IR: u16 = 0x0429;
pub const DESCRIPTION: &str = "Persian Phonetic (spike)";

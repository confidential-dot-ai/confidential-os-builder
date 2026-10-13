//! The hash algorithms measured boot logs use: SHA-384 for TDX RTMRs and
//! SHA-256 for the vTPM PCR bank cloud verifiers read.

use sha2::digest::DynDigest;
use sha2::{Digest, Sha256, Sha384};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlg {
    Sha256,
    Sha384,
}

impl HashAlg {
    pub(crate) fn hasher(self) -> Box<dyn DynDigest> {
        match self {
            HashAlg::Sha256 => Box::new(Sha256::new()),
            HashAlg::Sha384 => Box::new(Sha384::new()),
        }
    }

    pub fn hash(self, data: &[u8]) -> Vec<u8> {
        let mut hasher = self.hasher();
        hasher.update(data);
        hasher.finalize().to_vec()
    }

    /// Extend a register that starts at zero with each event digest in turn:
    /// `R = H(R || digest)`, as TDX RTMRs and TPM PCRs do.
    pub fn extend(self, events: &[Vec<u8>]) -> Vec<u8> {
        let mut register = vec![0u8; self.hasher().output_size()];
        for event in events {
            register = self.hash(&[register.as_slice(), event].concat());
        }
        register
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extend_matches_the_rtmr_helper() {
        let events = vec![HashAlg::Sha384.hash(b"a"), HashAlg::Sha384.hash(b"b")];
        let mut expected = vec![0u8; 48];
        for e in &events {
            expected = crate::rtmr::rtmr_extend(&expected, e);
        }
        assert_eq!(HashAlg::Sha384.extend(&events), expected);
        assert_eq!(HashAlg::Sha256.extend(&[]), vec![0u8; 32]);
    }
}

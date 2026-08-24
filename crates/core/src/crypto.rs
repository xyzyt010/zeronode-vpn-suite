use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireguardKeyMaterial {
    pub public_key: String,
    pub private_key: String,
}

impl WireguardKeyMaterial {
    pub fn is_complete(&self) -> bool {
        !self.public_key.trim().is_empty() && !self.private_key.trim().is_empty()
    }
}

pub fn generate_key_material() -> WireguardKeyMaterial {
    let private = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&private);

    WireguardKeyMaterial {
        public_key: STANDARD_NO_PAD.encode(public.as_bytes()),
        private_key: STANDARD_NO_PAD.encode(private.to_bytes()),
    }
}

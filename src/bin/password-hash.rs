use std::io::stdin;

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString},
};

#[cfg(feature = "bin")]
use rand_core::OsRng;

#[cfg(feature = "bin")]
fn main() {
    let mut password = String::new();
    stdin().read_line(&mut password).unwrap();

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.trim().as_bytes(), &salt)
        .expect("password hashing should succeed");

    println!("{hash}");
}

#[cfg(not(feature = "bin"))]
fn main() {}

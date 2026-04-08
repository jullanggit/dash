use std::io::stdin;

#[cfg(feature = "login")]
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString},
};

#[cfg(feature = "bin")]
use rand_core::OsRng;

#[cfg(all(feature = "bin", feature = "login"))]
fn main() {
    let mut password = String::new();
    stdin().read_line(&mut password).unwrap();

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.trim().as_bytes(), &salt)
        .expect("password hashing should succeed");

    println!("{hash}");
}

#[cfg(not(all(feature = "bin", feature = "login")))]
fn main() {}

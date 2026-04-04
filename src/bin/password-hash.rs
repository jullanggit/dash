use std::io::stdin;

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString},
};
use rand_core::OsRng;

fn main() {
    let mut password = String::new();
    stdin().read_line(&mut password).unwrap();

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("password hashing should succeed");

    println!("{hash}");
}

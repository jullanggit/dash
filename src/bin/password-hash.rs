use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString},
};
use rand_core::OsRng;

fn main() {
    let mut args = std::env::args();
    let program = args.next().unwrap_or_else(|| "password-hash".to_string());

    if let Some(password) = args.next()
        && args.next().is_none()
    {
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .expect("password hashing should succeed");

        println!("{hash}");
    } else {
        eprintln!("usage: {program} <password>");
        std::process::exit(2);
    };
}

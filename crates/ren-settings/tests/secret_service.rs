// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Secret Service implementation against a real Secret Service. Needs
//! an unlocked keyring on the session bus, so it's ignored by default; run
//! it with `just test-secret-service`, which starts a private session bus
//! and GNOME Keyring for it.

use ren_settings::{Account, SecretService, SecretStore, app_password};

#[test]
#[ignore = "needs a Secret Service: just test-secret-service"]
fn round_trip() {
    println!("Secret Service: {}", SecretService::provider().unwrap());
    let service = SecretService::connect().unwrap();
    let account = Account {
        server: "https://ren-test.invalid".to_owned(),
        user: format!("test-{}", std::process::id()),
        password_command: None,
    };
    let other = Account {
        user: format!("{}-other", account.user),
        ..account.clone()
    };

    assert_eq!(service.password(&account), Ok(None));
    service.set_password(&account, "first").unwrap();
    service.set_password(&other, "other").unwrap();
    assert_eq!(service.password(&account), Ok(Some("first".to_owned())));
    // Replaced, not added.
    service.set_password(&account, "sëcond").unwrap();
    assert_eq!(service.password(&account), Ok(Some("sëcond".to_owned())));
    assert_eq!(
        app_password(&account, None, SecretService::connect).map(|(password, _)| password),
        Ok("sëcond".to_owned())
    );

    service.delete_password(&account).unwrap();
    service.delete_password(&account).unwrap();
    assert_eq!(service.password(&account), Ok(None));
    assert_eq!(service.password(&other), Ok(Some("other".to_owned())));
    service.delete_password(&other).unwrap();
}

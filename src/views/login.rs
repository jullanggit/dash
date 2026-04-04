use crate::{Route, auth};
use dioxus::prelude::*;

#[component]
pub fn Login() -> Element {
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let navigator = use_navigator();

    rsx! {
        div { style: "display: flex; min-height: 100vh; align-items: center; justify-content: center; padding: 24px;",
            form {
                style: "display: flex; flex-direction: column; gap: 12px; width: min(100%, 320px);",
                onsubmit: move |event| {
                    event.prevent_default();
                    let submitted_password = password();
                    let nav = navigator.clone();
                    async move {
                        match auth::login(submitted_password).await {
                            Ok(()) => {
                                error.set(None);
                                nav.push(Route::Home {});
                            }
                            Err(e) => error.set(Some(format!("unauthenticated: {e}"))),
                        }
                    }
                },
                h1 { "Login" }
                input {
                    r#type: "password",
                    value: "{password}",
                    oninput: move |event| password.set(event.value()),
                    placeholder: "Password",
                }
                button { r#type: "submit", "Login" }
                if let Some(error) = error() {
                    p { "{error}" }
                }
            }
        }
    }
}

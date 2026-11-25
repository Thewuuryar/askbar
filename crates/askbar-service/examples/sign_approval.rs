//! Smoke helper (P5A.23): makes a keypair, and signs one tool action. Not part of any service.
//!
//!   sign_approval generate                         prints the PKCS#8 private key (base64), then the public key (hex)
//!   sign_approval sign USER TASK ONE_TIME_ID EXPIRES   reads the action JSON on stdin and the key from $APPROVAL_KEY, prints the Approval JSON
use askbar_proto::approval::ToolAction;
use askbar_service::approval::{generate_keypair, sign};
use base64::{engine::general_purpose::STANDARD, Engine};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("generate") => {
            let (private, public) = generate_keypair().expect("a keypair");
            println!("{}", STANDARD.encode(private));
            println!("{}", public.iter().map(|b| format!("{b:02x}")).collect::<String>());
        }
        Some("sign") if a.len() == 6 => {
            let action: ToolAction = serde_json::from_reader(std::io::stdin()).expect("an action on stdin");
            let key = STANDARD.decode(std::env::var("APPROVAL_KEY").expect("APPROVAL_KEY")).expect("base64 key");
            let approval = sign(&action, &a[3], &a[2], &a[4], a[5].parse().expect("expires"), &key).expect("signs");
            println!("{}", serde_json::to_string(&approval).expect("json"));
        }
        _ => {
            eprintln!("usage: sign_approval generate | sign USER TASK ONE_TIME_ID EXPIRES");
            std::process::exit(2);
        }
    }
}

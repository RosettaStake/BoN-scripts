use std::io::Write;

/// Wait for user confirmation before proceeding with transaction broadcast.
pub fn wait_for_user_confirmation() {
    println!("\n{}", "=".repeat(60));
    println!("✅ ALL TRANSACTIONS GENERATED AND SEQUENCED IN MEMORY");
    println!("{}", "=".repeat(60));
    loop {
        print!("Ready to BLAST? Press 'Y' then Enter to begin broadcasting: ");
        std::io::stdout().flush().ok();
        let mut input = String::new();
        match std::io::stdin().read_line(&mut input) {
            Ok(0) => break, // EOF
            Ok(_) => {
                if input.trim().eq_ignore_ascii_case("y") {
                    println!("🚀 BLAST INITIATED 🚀\n");
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

use switchboard_macros::route;

fn main() {
    let _ = route!(get, "/", (), (), prefixes = ["/hu"]);
    let _ = route!(get, "/", (), (), prefixes = ["hu/:locale"]);
}

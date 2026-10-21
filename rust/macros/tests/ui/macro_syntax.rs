use switchboard_macros::route;

fn main() {
    let _ = route!(trace, "/", (), ());
    let _ = route!(get, PATH, (), ());
    let _ = route!(get, concat!("/", "items"), (), ());
    let _ = route!(post, "/", (), String);
    let _ = route!(get, "/", (), String => String);
}

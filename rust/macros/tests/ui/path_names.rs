use switchboard_macros::route;

fn main() {
    let _ = route!(get, "/items/:", (), ());
    let _ = route!(get, "/items/:bad-name", (), ());
    let _ = route!(get, "/:name/:name?", (), ());
    let _ = route!(get, "/:self", (), ());
    let _ = route!(get, "/:Self", (), ());
    let _ = route!(get, "/:super", (), ());
    let _ = route!(get, "/:crate", (), ());
}

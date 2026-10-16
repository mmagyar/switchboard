use switchboard_macros::route;

fn main() {
    let _ = route!(get, "/:id?/:name", (), ());
    let _ = route!(get, "/:id?/fixed", (), ());
}

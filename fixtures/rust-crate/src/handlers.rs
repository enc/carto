use crate::orders::parse_order;

pub fn handle(input: &str) {
    let order = parse_order(input);
    log_order(order.id);
    order.summary();
    audit_order(order.id);
    unknown_external_call();
}

fn log_order(id: u64) {
    println!("order {id}");
}

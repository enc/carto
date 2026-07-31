pub struct Order {
    pub id: u64,
}

impl Order {
    pub fn summary(&self) -> String {
        format!("order #{}", self.id)
    }
}

pub fn parse_order(input: &str) -> Order {
    validate(input);
    Order { id: 1 }
}

fn validate(input: &str) -> bool {
    !input.is_empty()
}

pub fn audit_order(id: u64) {
    println!("auditing {id}");
}

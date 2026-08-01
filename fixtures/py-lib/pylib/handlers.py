from .orders import parse_order


def handle(input):
    order = parse_order(input)
    log_order(order.id)
    order.summary()
    audit_order(order.id)
    unknown_external_call()
    input.strip()


def log_order(id):
    print(f"order {id}")

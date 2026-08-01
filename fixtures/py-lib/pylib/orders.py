class Order:
    def __init__(self, id):
        self.id = id

    def summary(self):
        return f"order #{self.id}"


def parse_order(input):
    validate(input)
    return Order(1)


def validate(input):
    return input != ""


def audit_order(id):
    print(f"auditing {id}")

export interface Order {
    id: string;
}

export function parseOrder(input: string): Order {
    validate(input);
    return { id: input };
}

export function validate(input: string): boolean {
    return input !== '';
}

export const auditOrder = (id: string) => {
    console.log(`auditing ${id}`);
};

function internalHelper(): number {
    return 1;
}

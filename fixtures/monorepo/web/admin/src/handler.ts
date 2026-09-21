// A fourth same-named symbol -- admin's own Handler (see
// services/orders/handler.go's comment for the full picture).

export class Handler {
    handle(input: string): string {
        return `admin:${input}`;
    }
}

// Module-level setup code -- no enclosing symbol at all, the same
// shape ADR-0031 resolves at file scope instead of dropping (C#'s
// Program.cs top-level statements, TS/JS module-level setup calls).
// Exercises that fallback path under a component: the resolved edge
// must attach to this File node, still carrying this file's own
// component label.
registerHandler(new Handler());

function registerHandler(h: Handler): void {
    console.log(h.handle("boot"));
}

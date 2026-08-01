import { parseOrder, validate as v } from '../orders';
import { logOrder } from '../shared/logging';
import { Logger } from '@scope/pkg/logging';

export class Handler {
    handle(input: string) {
        const order = parseOrder(input);
        v(input);
        logOrder(order.id);
        auditOrder(order.id);
        internalHelper();
        unknownExternalCall();
    }
}

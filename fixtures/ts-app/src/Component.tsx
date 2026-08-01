import { legacyHelper } from './legacy';

export function Component() {
    const value = legacyHelper(1);
    return <div>{value}</div>;
}

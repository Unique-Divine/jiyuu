class Widget {
    render(): string { return "widget"; }
}

class Gadget {
    spin(): number { return 1; }
}

class Box<T> {
    constructor(private value: T) {}
    get(): T { return this.value; }
}

function generic(box: Box<Widget>): void {
    box.get();
}

function wrapped(item: Readonly<Widget>): void {
    item.render();
}

function union(item: Widget | Gadget): void {
    item.render();
}

function nullable(item: Widget | null): void {
    item.render();
}

function intersection(item: Widget & Gadget): void {
    item.spin();
}

function array(items: Widget[]): void {
    items.pop().render();
}

function arrayGeneric(items: Array<Gadget>): void {
    for (const item of items) {
        item.spin();
    }
}

function tuple(pair: [Widget, Gadget]): void {
    const [first, second] = pair;
    second.spin();
}

function callback(make: () => Widget): void {
    make().render();
}

function callbackArg(make: (seed: number, label?: string) => Gadget): void {
    make(1).spin();
}

function keyed(item: Widget, key: keyof Widget): void {
    item.render();
}

const sample: Gadget = new Gadget();

function query(item: typeof sample): void {
    item.spin();
}

type Unwrap<T> = T extends Promise<infer R> ? R : T;

function conditional(item: Unwrap<Promise<Widget>>): void {
    item.render();
}

function returned(): Box<Gadget> {
    return new Box(new Gadget());
}

function chained(): void {
    returned().get();
}

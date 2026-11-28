class ViewData {
    query(sql: string): string { return sql; }
}

class Panel<T> {
    constructor(private data: T) {}
    item(): T { return this.data; }
}

export function renderTyped(data: ViewData | undefined) {
    return <p>{data.query("typed")}</p>;
}

export function renderPanel(panel: Panel<ViewData>) {
    return <p>{panel.item().query("panel")}</p>;
}

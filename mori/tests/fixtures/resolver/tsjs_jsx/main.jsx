class JsxSession {
    query(sql) { return sql; }
}

/** @param {JsxSession} session */
function renderLegacy(session) {
    return <p>{session.query("jsx")}</p>;
}

/** @param {(id: number) => JsxSession} open */
function renderOpened(open) {
    return <p>{open(1).query("opened")}</p>;
}

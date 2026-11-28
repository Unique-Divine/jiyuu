class Widget {
    render() { return "widget"; }
}

class Gadget {
    spin() { return 1; }
}

/** @param {Widget} item */
function named(item) {
    item.render();
}

/** @param {Widget | Gadget} item */
function union(item) {
    item.render();
}

/** @param {Widget[]} items */
function array(items) {
    items.pop().render();
}

/** @param {Array<Gadget>} items */
function arrayGeneric(items) {
    for (const item of items) {
        item.spin();
    }
}

/** @param {() => Gadget} make */
function callback(make) {
    make().spin();
}

/** @returns {Widget} */
function build() {
    return new Widget();
}

function chained() {
    build().render();
}

/** @returns {Promise<Gadget>} */
async function fetchGadget() {
    return new Gadget();
}

async function awaited() {
    (await fetchGadget()).spin();
}

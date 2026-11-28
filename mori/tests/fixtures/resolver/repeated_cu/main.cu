struct Alpha { int render() { return 1; } };
struct Bravo { int render() { return 2; } };
int occurrence_probe() {
    Alpha alpha;
    Bravo bravo;
    return alpha.render() + bravo.render();
}

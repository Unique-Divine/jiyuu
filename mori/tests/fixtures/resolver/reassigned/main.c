static int alpha_target(void) { return 1; }
static int bravo_target(void) { return 2; }
int occurrence_probe(void) {
    int (*fp)(void) = alpha_target;
    int first = fp();
    fp = bravo_target;
    return first + fp();
}

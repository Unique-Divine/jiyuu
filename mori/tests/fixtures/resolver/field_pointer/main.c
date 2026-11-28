struct Ops {
    int (*shut)(int fd);
};
struct OtherOps {
    int (*shut)(int fd);
    int (*close)(int fd);
};
struct Dev {
    struct Ops *ops;
};
int occurrence_probe(struct Dev *d, struct OtherOps *other) {
    int first = d->ops->shut(3);
    return first + other->close(4);
}

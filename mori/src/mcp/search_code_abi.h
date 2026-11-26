/* Private ABI between the MCP search dispatcher and the Rust search policy. */
#ifndef CBM_SEARCH_CODE_ABI_H
#define CBM_SEARCH_CODE_ABI_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct {
    char *file;
    int line;
    char content[1024];
    size_t content_start_byte;
    size_t content_returned_bytes;
    size_t content_total_bytes;
    size_t match_start_byte;
    size_t match_end_byte;
    bool match_known;
    bool content_truncated;
} grep_match_t;

typedef struct {
    int64_t node_id;
    char *qualified_name;
    char label[64];
    char *file;
    int start_line;
    int end_line;
    int in_degree;
    int out_degree;
    int *match_lines;
    int match_count;
    int match_stored_count;
    int match_capacity;
    int last_match_line;
} search_result_t;

typedef struct {
    const char *pattern;
    const char *project;
    const char *root;
    const char *file_pattern;
    void *path_regex;
    int raw_offset;
    int raw_limit;
    size_t raw_content_offset;
    bool raw_content_offset_set;
    bool use_regex;
    bool debug;
} search_gather_args_t;

typedef struct {
    search_result_t *results;
    grep_match_t *raw;
    int result_count;
    int raw_count;
    int raw_stored_count;
    int raw_content_truncated;
    int grep_count;
    uint64_t scope_ms;
    uint64_t scan_ms;
    uint64_t enrich_ms;
    bool timed_out;
    char error[256];
} search_gathered_t;

/* Inputs are borrowed synchronously. All returned char* allocations use the
 * native allocator and are released by the native free callback. */
typedef struct {
    char *(*project)(const char *);
    char *(*root)(void *, const char *);
    char *(*project_error)(void *, const char *);
    void *(*regex_open)(const char *, int *);
    void (*regex_close)(void *);
    void (*gather)(void *, const search_gather_args_t *, search_gathered_t *);
    void (*release)(search_gathered_t *);
    char *(*source)(void *, const char *, const char *, int, int);
    char *(*text_result)(const char *, bool);
    char *(*copy)(const char *);
    void (*free_string)(char *);
    char *(*json_tree)(const char *);
    char *(*tree_table)(const char *, int, const char *const *, int, const char *const *,
                        const bool *, const bool *);
    uint64_t (*now_ms)(void);
    void (*log_slow)(uint64_t);
} search_native_ops_t;

char *mori_search_code(void *context, const char *args, const search_native_ops_t *ops);

#ifdef CBM_SEARCH_CODE_TEST_FIXTURES
char *mori_search_code_render_fixture(const search_gathered_t *data, bool json,
                                      const search_native_ops_t *ops);
#endif
#endif

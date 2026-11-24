/* Decode a query value for either UI transport. */
#ifndef CBM_UI_HTTP_QUERY_H
#define CBM_UI_HTTP_QUERY_H

#include <stdbool.h>
#include <stddef.h>
#include <string.h>

static inline int ui_http_hex_val(char ch) {
    if (ch >= '0' && ch <= '9')
        return ch - '0';
    if (ch >= 'a' && ch <= 'f')
        return ch - 'a' + 10;
    if (ch >= 'A' && ch <= 'F')
        return ch - 'A' + 10;
    return -1;
}

static inline bool cbm_ui_http_query_param(const char *query, const char *name, char *buf,
                                           int bufsz) {
    if (!query || !name || !buf || bufsz <= 0)
        return false;
    buf[0] = '\0';
    size_t name_len = strlen(name);

    const char *p = query;
    while (*p) {
        const char *pair_end = strchr(p, '&');
        if (!pair_end)
            pair_end = p + strlen(p);
        const char *eq = memchr(p, '=', (size_t)(pair_end - p));
        size_t klen = eq ? (size_t)(eq - p) : (size_t)(pair_end - p);

        if (klen == name_len && memcmp(p, name, name_len) == 0) {
            if (!eq)
                return false; /* present but no value */
            /* Percent-decode the value into buf. */
            const char *v = eq + 1;
            int out = 0;
            while (v < pair_end) {
                char ch = *v;
                if (ch == '+') {
                    ch = ' ';
                    v++;
                } else if (ch == '%') {
                    if (pair_end - v < 3) /* needs two hex digits */
                        return false;
                    int hi = ui_http_hex_val(v[1]);
                    int lo = ui_http_hex_val(v[2]);
                    if (hi < 0 || lo < 0)
                        return false;
                    ch = (char)((hi << 4) | lo);
                    if (ch == '\0')
                        return false; /* decoded NUL never allowed */
                    v += 3;
                } else {
                    v++;
                }
                if (out >= bufsz - 1) {
                    buf[0] = '\0';
                    return false; /* oversize — reject, never truncate */
                }
                buf[out++] = ch;
            }
            buf[out] = '\0';
            return out > 0;
        }
        p = (*pair_end == '&') ? pair_end + 1 : pair_end;
    }
    return false;
}

#endif

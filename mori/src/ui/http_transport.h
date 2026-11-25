/* Rust HTTP transport ABI. The UI host owns the handle until run returns. */
#ifndef CBM_UI_HTTP_TRANSPORT_H
#define CBM_UI_HTTP_TRANSPORT_H

#include "foundation/constants.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define CBM_HTTP_MAX_HEAD (16 * 1024)
#define CBM_HTTP_MAX_BODY (1024 * 1024)
#define CBM_HTTP_NEED_MORE (-1)

typedef enum {
    CBM_HTTPD_ACTIVITY_IDLE = 0,
    CBM_HTTPD_ACTIVITY_READING_REQUEST = 1,
    CBM_HTTPD_ACTIVITY_RESPONDING = 2,
} cbm_httpd_activity_t;

/* The target and selected headers are raw bytes with a NUL terminator.
 * The transport owns the body through dispatch and the finish callback. */
typedef struct {
    char method[CBM_SZ_16];
    char path[CBM_SZ_2K];
    char query[CBM_SZ_2K];
    unsigned char http_minor;
    char origin[CBM_SZ_256];
    char host[CBM_SZ_256];
    char content_type[CBM_SZ_128];
    char accept_language[CBM_SZ_256];
    char *body;
    size_t body_len;
} cbm_http_req_t;

/* Deliver one reply on the dispatch thread. Copy headers and body before
 * returning if the transport will write them later. A zero deadline selects
 * the transport default. */
typedef void (*cbm_ui_http_reply_fn)(void *context, int status, const char *headers,
                                     const void *body, size_t body_len, int send_deadline_ms);

typedef struct mori_ui_http_transport mori_ui_http_transport_t;
typedef void (*mori_ui_http_dispatch_fn)(void *context, const cbm_http_req_t *req,
                                         cbm_ui_http_reply_fn reply, void *reply_context);
typedef void (*mori_ui_http_finish_fn)(void *context, const cbm_http_req_t *req, int status,
                                       int64_t elapsed_ms, size_t response_bytes);

mori_ui_http_transport_t *mori_ui_http_bind(int port);
int mori_ui_http_port(const mori_ui_http_transport_t *transport);
void mori_ui_http_run(const mori_ui_http_transport_t *transport, void *context,
                      mori_ui_http_dispatch_fn dispatch, mori_ui_http_finish_fn finish);
void mori_ui_http_stop(const mori_ui_http_transport_t *transport);
bool mori_ui_http_free(mori_ui_http_transport_t *transport);
int mori_ui_http_activity(const mori_ui_http_transport_t *transport);
void mori_ui_http_set_receive_ms(const mori_ui_http_transport_t *transport, int ms);

/* Test seams use the same framing and socket code as the production hosts. */
int mori_ui_http_parse_head_for_test(const char *data, size_t len, cbm_http_req_t *request,
                                     size_t *body_offset, size_t *content_length);
void mori_ui_http_set_send_options_for_test(const mori_ui_http_transport_t *transport,
                                            int buffer_bytes, int deadline_ms);

#endif

/* Rust HTTP transport ABI. The UI host owns the handle until run returns. */
#ifndef CBM_UI_HTTP_TRANSPORT_H
#define CBM_UI_HTTP_TRANSPORT_H

#include "ui/http_server.h"

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

#endif

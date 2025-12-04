#include <stdlib.h>
#include <stdio.h>

#include "cluster/tests/components/server.h"

void this_old_trope_hello_world(
    this_old_trope_hello_request_t *request,
    this_old_trope_context_t *context,
    this_old_trope_hello_response_t *response
) {
    // "Hello, !" is 9 bytes (including the terminating NULL).
    char * message = (char *)malloc(request->name.len + 9);
    sprintf(message, "Hello, %s!", request->name.ptr);
    server_string_set(&response->message, message);
}

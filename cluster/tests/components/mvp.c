#include <stdlib.h>
#include <stdio.h>

#include "cluster/tests/components/server.h"

void this_old_trope_hello_world(
    this_old_trope_hello_request_t *request,
    this_old_trope_context_t *context,
    this_old_trope_hello_response_t *response
) {
    // "Hello, !" is 9 bytes (including the terminating NULL).
    size_t message_length = request->name.len + 9;
    char * message = (char *)malloc(message_length);
    snprintf(message, message_length, "Hello, %s!", request->name.ptr);
    // This transfers "ownership" of the string, so we don't have to free it.
    server_string_set(&response->message, message);
}

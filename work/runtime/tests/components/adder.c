#include "work/runtime/tests/components/adder_service.h"

void adder_service_add_floats(
    adder_service_context_t *ctx,
    foo_bar_types_add_floats_request_t *request,
    foo_bar_types_add_floats_response_t *response
) {
    response->result = request->x + request->y;
}
load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")

def _delegating_string_flag_impl(ctx):
    value = ctx.attr.delegate[BuildSettingInfo].value
    if not value or ctx.build_setting_value != ctx.attr.build_setting_default:
        value = ctx.build_setting_value
    return [BuildSettingInfo(value = value)] + _get_template_variable_info(ctx, value)

delegating_string_flag = rule(
    implementation = _delegating_string_flag_impl,
    build_setting = config.string(flag = True),
    attrs = {
        "delegate": attr.label(
            doc = "Use the value of this build setting by default, unless its empty.",
            mandatory = True,
            providers = [BuildSettingInfo],
        ),
        "make_variable": attr.string(
            doc = "If set, the build setting's value will be available as a Make variable with this " +
                  "name in the attributes of rules that list this build setting in their 'toolchains' " +
                  "attribute.",
        ),
    },
    doc = "A string-typed build setting that can be set on the command line." +
          " If unset, defaults to the value of a delegate build setting," +
          " but only if that value is non-empty." +
          "" +
          "This allows expressing multiple build settings, with distinct default values," +
          " that can be simultaneously overridden by a single flag" +
          " while all being individually overridable too.",
)

# The following helper functions are adapted from Skylib.
# https://github.com/bazelbuild/bazel-skylib/blob/1.9.0/rules/common_settings.bzl

def _get_template_variable_info(ctx, value):
    make_variable = getattr(ctx.attr, "make_variable", None)
    if not make_variable:
        return []

    if not all([_is_valid_make_variable_char(c) for c in make_variable.elems()]):
        fail("Error setting {}:".format(_no_at_str(ctx.label)) +
             " invalid make variable name '{}'.".format(make_variable) +
             " Make variable names may only contain uppercase letters, digits, and underscores.")

    return [platform_common.TemplateVariableInfo({make_variable: str(value)})]

def _is_valid_make_variable_char(c):
    # Restrict make variable names for consistency with predefined ones. There are no enforced
    # restrictions on make variable names, but when they contain e.g. spaces or braces, they
    # aren't expanded by e.g. cc_binary.
    return c == "_" or c.isdigit() or (c.isalpha() and c.isupper())

def _no_at_str(label):
    """Strips any leading '@'s for labels in the main repo, so that the error string is more friendly."""
    s = str(label)
    if s.startswith("@@//"):
        return s[2:]
    if s.startswith("@//"):
        return s[1:]
    return s

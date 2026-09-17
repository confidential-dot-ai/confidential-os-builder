/* Diagnostic kernels only: call injection APIs with benign, valid AML fixtures. */
#include <linux/acpi.h>
#include <linux/init.h>
#include <linux/printk.h>
#include <linux/slab.h>
#include <confos-aml-test-data.h>
#include "acpica/accommon.h"
#include "acpica/acnamesp.h"
#include "acpica/actables.h"

static int __init confos_aml_probe(void)
{
    acpi_status status;
    u32 index;
    unsigned long long value = 0;
    struct acpi_buffer out = { ACPI_ALLOCATE_BUFFER, NULL };
    union acpi_object *obj;

    status = acpi_install_method(test_method_aml);
    pr_info("AMLTEST: acpi_install_method=%s\n", acpi_format_exception(status));
    status = acpi_evaluate_integer(NULL, "\\TSTI", NULL, &value);
    pr_info("AMLTEST: installed-method=%s\n", acpi_format_exception(status));
    if (ACPI_SUCCESS(status))
        pr_info("AMLTEST: installed-method-value=0x%llx\n", value);

    /* The control proves method injection without unloading its namespace. */
    if (!IS_ENABLED(CONFIG_ACPI_TRUSTED_AML))
        return 0;

    status = acpi_load_table((struct acpi_table_header *)test_aml, &index);
    pr_info("AMLTEST: acpi_load_table=%s\n", acpi_format_exception(status));
    status = acpi_evaluate_object(NULL, "\\LDBF", NULL, NULL);
    pr_info("AMLTEST: Load(buffer)=%s\n", acpi_format_exception(status));
    /* The host SSDT never enters the root table list, so LoadTable finds
     * nothing: it returns Integer 0 with AE_OK rather than a table handle. */
    status = acpi_evaluate_object(NULL, "\\LDTB", NULL, &out);
    obj = out.pointer;
    if (ACPI_SUCCESS(status) && obj && obj->type == ACPI_TYPE_INTEGER && !obj->integer.value)
        pr_info("AMLTEST: LoadTable=no-table\n");
    else
        pr_info("AMLTEST: LoadTable=%s\n", acpi_format_exception(status));
    kfree(out.pointer);
    status = acpi_ns_load_table(acpi_gbl_dsdt_index, acpi_gbl_root_node);
    pr_info("AMLTEST: repeated-primary=%s\n", acpi_format_exception(status));
    status = acpi_tb_unload_table(acpi_gbl_dsdt_index);
    pr_info("AMLTEST: unload-primary=%s\n", acpi_format_exception(status));
    status = acpi_ns_load_table(acpi_gbl_dsdt_index, acpi_gbl_root_node);
    pr_info("AMLTEST: reload-primary=%s\n", acpi_format_exception(status));
    return 0;
}
late_initcall(confos_aml_probe);

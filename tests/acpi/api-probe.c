/* Diagnostic kernel only: call injection APIs with a benign, valid AML table. */
#include <linux/acpi.h>
#include <linux/init.h>
#include <linux/printk.h>
#include <confos-aml-test-data.h>
#include "acpica/accommon.h"
#include "acpica/acnamesp.h"
#include "acpica/actables.h"

static int __init confos_aml_probe(void)
{
    acpi_status status;
    u32 index;

    status = acpi_load_table((struct acpi_table_header *)test_aml, &index);
    pr_info("AMLTEST: acpi_load_table=%s\n", acpi_format_exception(status));
    status = acpi_install_method(test_aml);
    pr_info("AMLTEST: acpi_install_method=%s\n", acpi_format_exception(status));
    status = acpi_evaluate_object(NULL, "\\LDBF", NULL, NULL);
    pr_info("AMLTEST: Load(buffer)=%s\n", acpi_format_exception(status));
    status = acpi_evaluate_object(NULL, "\\LDTB", NULL, NULL);
    pr_info("AMLTEST: LoadTable=%s\n", acpi_format_exception(status));
    status = acpi_ns_load_table(acpi_gbl_dsdt_index, acpi_gbl_root_node);
    pr_info("AMLTEST: repeated-primary=%s\n", acpi_format_exception(status));
    status = acpi_tb_unload_table(acpi_gbl_dsdt_index);
    pr_info("AMLTEST: unload-primary=%s\n", acpi_format_exception(status));
    status = acpi_ns_load_table(acpi_gbl_dsdt_index, acpi_gbl_root_node);
    pr_info("AMLTEST: reload-primary=%s\n", acpi_format_exception(status));
    return 0;
}
late_initcall(confos_aml_probe);

// Mock stub implementation of libkrun C FFI for build and test environments
// where libkrun is not natively installed.

int krun_create_ctx(void) {
    return 0;
}

int krun_free_ctx(unsigned int ctx_id) {
    (void)ctx_id;
    return 0;
}

int krun_set_vm_config(unsigned int ctx_id, unsigned char num_vcpus, unsigned int ram_mib) {
    (void)ctx_id;
    (void)num_vcpus;
    (void)ram_mib;
    return 0;
}

int krun_set_root(unsigned int ctx_id, const char *root_path) {
    (void)ctx_id;
    (void)root_path;
    return 0;
}

int krun_set_exec(
    unsigned int ctx_id,
    const char *exec_path,
    const char *const *argv,
    const char *const *envp
) {
    (void)ctx_id;
    (void)exec_path;
    (void)argv;
    (void)envp;
    return 0;
}

int krun_set_workdir(unsigned int ctx_id, const char *workdir) {
    (void)ctx_id;
    (void)workdir;
    return 0;
}

int krun_set_env(unsigned int ctx_id, const char *const *env) {
    (void)ctx_id;
    (void)env;
    return 0;
}

int krun_set_port_map(unsigned int ctx_id, const char *const *port_map) {
    (void)ctx_id;
    (void)port_map;
    return 0;
}

int krun_set_console_output(unsigned int ctx_id, const char *filepath) {
    (void)ctx_id;
    (void)filepath;
    return 0;
}

int krun_set_log_level(unsigned int level) {
    (void)level;
    return 0;
}

int krun_set_rlimits(unsigned int ctx_id, const char *rlimits) {
    (void)ctx_id;
    (void)rlimits;
    return 0;
}

int krun_set_gpu_options(unsigned int ctx_id, unsigned int virgl_flags) {
    (void)ctx_id;
    (void)virgl_flags;
    return 0;
}

int krun_set_gpu_options2(unsigned int ctx_id, unsigned int virgl_flags, unsigned long long shm_size) {
    (void)ctx_id;
    (void)virgl_flags;
    (void)shm_size;
    return 0;
}

int krun_add_virtiofs(unsigned int ctx_id, const char *tag, const char *path) {
    (void)ctx_id;
    (void)tag;
    (void)path;
    return 0;
}

int krun_add_virtiofs2(
    unsigned int ctx_id,
    const char *tag,
    const char *path,
    unsigned int flags
) {
    (void)ctx_id;
    (void)tag;
    (void)path;
    (void)flags;
    return 0;
}

int krun_add_net_unixstream(
    unsigned int ctx_id,
    const char *c_path,
    int fd,
    unsigned char *c_mac,
    unsigned int features,
    unsigned int flags
) {
    (void)ctx_id;
    (void)c_path;
    (void)fd;
    (void)c_mac;
    (void)features;
    (void)flags;
    return 0;
}

int krun_add_vsock_port(unsigned int ctx_id, unsigned int port, const char *path) {
    (void)ctx_id;
    (void)port;
    (void)path;
    return 0;
}

_Bool krun_check_nested_virt(void) {
    return 0;
}

int krun_add_disk(unsigned int ctx_id, const char *block_id, const char *disk_path, _Bool read_only) {
    (void)ctx_id; (void)block_id; (void)disk_path; (void)read_only;
    return 0;
}

int krun_set_kernel(unsigned int ctx_id, const char *kernel_path, unsigned int kernel_format, const char *initramfs, const char *cmdline) {
    (void)ctx_id; (void)kernel_path; (void)kernel_format; (void)initramfs; (void)cmdline;
    return 0;
}

int krun_set_firmware(unsigned int ctx_id, const char *firmware_path) {
    (void)ctx_id; (void)firmware_path;
    return 0;
}

int krun_disable_implicit_console(unsigned int ctx_id) {
    (void)ctx_id;
    return 0;
}

int krun_add_serial_console_default(unsigned int ctx_id, int input_fd, int output_fd) {
    (void)ctx_id; (void)input_fd; (void)output_fd;
    return 0;
}

int krun_start_enter(unsigned int ctx_id) {
    (void)ctx_id;
    return 0;
}


// rad-sandbox: runs one command in a libkrun microVM on this terminal.
//
//   rad-sandbox [--cpus N] [--mem MiB] [--disk IMG | --root DIR] [--data IMG] [--net SOCKET]
//               [--share TAG=DIR]... [--vsock PORT=SOCKET]... [--workdir DIR] [--env KEY=VALUE]...
//               -- CMD [ARGS...]
//
// --disk   boots from an ext4 disk image (the sandbox's own drive)
// --root   boots from a host directory (used for RAD's small installer VM)
// --data   attaches a disk image without booting from it (/dev/vda; the installer formats it)
// --net    attaches a virtio-net card to gvproxy's unixgram socket (vfkit protocol, DHCP).
//          Without it the VM has no network at all: libkrun's implicit host-socket
//          forwarding (TSI) is always switched off, so the guest can never reach this
//          computer's localhost.
// --share  exposes a host directory as virtiofs tag TAG (the guest mounts it itself).
// --vsock  connects guest vsock port PORT to a unix socket on the host that this program
//          listens on: host programs connect to SOCKET to reach a server in the guest (RAD's
//          containers use it for their agent). This is plain vsock, never TSI.
//
// The process exits with the guest command's exit code.

#include <dlfcn.h>
#include <libgen.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <libkrun.h>

#define MAX_LIST 32

static void die(const char *what, int err) {
    fprintf(stderr, "rad-sandbox: %s failed (%d)\n", what, err);
    exit(125);
}

// libkrun dlopen()s "libkrunfw.5.dylib" by name. Loading it first from next to this program
// (or the app bundle's Frameworks folder) makes that lookup find the copy we ship.
static void preload_kernel(void) {
    char exe[PATH_MAX];
    uint32_t size = sizeof(exe);
    if (_NSGetExecutablePath(exe, &size) != 0) return;
    char real[PATH_MAX];
    if (!realpath(exe, real)) return;
    char *dir = dirname(real);
    const char *tries[] = { "%s/libkrunfw.5.dylib", "%s/../Frameworks/libkrunfw.5.dylib", NULL };
    for (int i = 0; tries[i]; i++) {
        char path[PATH_MAX];
        snprintf(path, sizeof(path), tries[i], dir);
        if (dlopen(path, RTLD_NOW | RTLD_GLOBAL)) return;
    }
}

int main(int argc, char **argv) {
    int cpus = 2, mem = 1024;
    const char *disk = NULL, *root = NULL, *data = NULL, *net = NULL, *workdir = "/root";
    const char *shares[MAX_LIST], *envs[MAX_LIST + 1], *vsocks[MAX_LIST];
    int nshares = 0, nenvs = 0, nvsocks = 0;
    int i = 1;
    while (i < argc) {
        const char *a = argv[i];
        const char *v = i + 1 < argc ? argv[i + 1] : NULL;
        if (!strcmp(a, "--")) { i++; break; }
        if (!v) { fprintf(stderr, "rad-sandbox: %s needs a value\n", a); return 2; }
        if (!strcmp(a, "--cpus")) cpus = atoi(v);
        else if (!strcmp(a, "--mem")) mem = atoi(v);
        else if (!strcmp(a, "--disk")) disk = v;
        else if (!strcmp(a, "--root")) root = v;
        else if (!strcmp(a, "--data")) data = v;
        else if (!strcmp(a, "--net")) net = v;
        else if (!strcmp(a, "--workdir")) workdir = v;
        else if (!strcmp(a, "--share") && nshares < MAX_LIST) shares[nshares++] = v;
        else if (!strcmp(a, "--env") && nenvs < MAX_LIST) envs[nenvs++] = v;
        else if (!strcmp(a, "--vsock") && nvsocks < MAX_LIST) vsocks[nvsocks++] = v;
        else { fprintf(stderr, "rad-sandbox: unknown option %s\n", a); return 2; }
        i += 2;
    }
    if (i >= argc || (!disk && !root)) {
        fprintf(stderr, "usage: rad-sandbox [--disk IMG | --root DIR] [options] -- CMD [ARGS...]\n");
        return 2;
    }
    envs[nenvs] = NULL;

    preload_kernel();
    const char *log = getenv("RAD_SANDBOX_LOG"); // 0 off … 5 trace, for debugging
    krun_set_log_level(log ? (uint32_t)atoi(log) : 0);
    int ctx = krun_create_ctx(), r;
    if (ctx < 0) die("krun_create_ctx", ctx);
    if (cpus < 1) cpus = 1;
    if (cpus > 8) cpus = 8;
    if (mem < 256) mem = 256;
    if ((r = krun_set_vm_config(ctx, (uint8_t)cpus, (uint32_t)mem))) die("krun_set_vm_config", r);
    // Never forward guest sockets to the host (TSI): networking only through --net.
    if ((r = krun_disable_implicit_vsock(ctx))) die("krun_disable_implicit_vsock", r);

    if (disk) {
        if ((r = krun_add_disk2(ctx, "root", disk, KRUN_DISK_FORMAT_RAW, false))) die("krun_add_disk2", r);
        if ((r = krun_set_root_disk_remount(ctx, "/dev/vda", "ext4", NULL))) die("krun_set_root_disk_remount", r);
    } else if ((r = krun_set_root(ctx, root))) {
        die("krun_set_root", r);
    }
    if (data && !disk && (r = krun_add_disk2(ctx, "data", data, KRUN_DISK_FORMAT_RAW, false))) die("krun_add_disk2", r);

    for (int s = 0; s < nshares; s++) {
        char tag[64];
        const char *eq = strchr(shares[s], '=');
        if (!eq || eq == shares[s] || (size_t)(eq - shares[s]) >= sizeof(tag)) { fprintf(stderr, "rad-sandbox: bad --share %s\n", shares[s]); return 2; }
        memcpy(tag, shares[s], eq - shares[s]);
        tag[eq - shares[s]] = 0;
        if ((r = krun_add_virtiofs(ctx, tag, eq + 1))) die("krun_add_virtiofs", r);
    }

    if (nvsocks) {
        // An explicit vsock device without TSI: only the listed ports, nothing hijacked.
        if ((r = krun_add_vsock(ctx, 0))) die("krun_add_vsock", r);
        for (int s = 0; s < nvsocks; s++) {
            char *end;
            long port = strtol(vsocks[s], &end, 10);
            if (*end != '=' || port <= 0 || port > 0xffffffffL || !end[1]) { fprintf(stderr, "rad-sandbox: bad --vsock %s\n", vsocks[s]); return 2; }
            if ((r = krun_add_vsock_port2(ctx, (uint32_t)port, end + 1, true))) die("krun_add_vsock_port2", r);
        }
    }

    if (net) {
        uint8_t mac[6] = { 0x5a, 0x94, 0xef, 0xe4, 0x0c, 0xee };
        if ((r = krun_add_net_unixgram(ctx, net, -1, mac, COMPAT_NET_FEATURES, NET_FLAG_VFKIT | NET_FLAG_DHCP_CLIENT)))
            die("krun_add_net_unixgram", r);
    }

    if ((r = krun_set_workdir(ctx, workdir))) die("krun_set_workdir", r);
    if ((r = krun_set_exec(ctx, argv[i], (const char *const *)&argv[i + 1], envs))) die("krun_set_exec", r);
    r = krun_start_enter(ctx); // only returns if the VM couldn't start
    die("krun_start_enter", r);
    return 125;
}

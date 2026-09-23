/* pam_sentinel.c — Sentinel Recreated PAM bridge.
 * Zero biometric logic. Calls com.sentinel.Sentinel.Authenticate via libdbus-1.
 * Spec: docs/PAM_INTEGRATION.md  Constraint: < 200 lines C99. */

#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/types.h>
#define PAM_SM_AUTH
#include <security/pam_modules.h>
#include <dbus/dbus.h>

#define SENTINEL_BUS  "com.sentinel.Sentinel"
#define SENTINEL_PATH "/com/sentinel/Sentinel"
#define SENTINEL_IFACE "com.sentinel.Sentinel"
#define SENTINEL_DBUS_TIMEOUT_MS 5000

static int sentinel_reachable(DBusConnection *c)
{
    DBusMessage *m = dbus_message_new_method_call(
        "org.freedesktop.DBus", "/org/freedesktop/DBus",
        "org.freedesktop.DBus", "NameHasOwner");
    if (!m) return 0;
    const char *name = SENTINEL_BUS;
    if (!dbus_message_append_args(m, DBUS_TYPE_STRING, &name,
                                  DBUS_TYPE_INVALID)) {
        dbus_message_unref(m); return 0;
    }
    DBusError e; dbus_error_init(&e);
    DBusMessage *r = dbus_connection_send_with_reply_and_block(c, m, 2000, &e);
    dbus_message_unref(m);
    if (dbus_error_is_set(&e) || !r) { dbus_error_free(&e); return 0; }
    dbus_bool_t has = FALSE;
    dbus_message_get_args(r, &e, DBUS_TYPE_BOOLEAN, &has, DBUS_TYPE_INVALID);
    dbus_message_unref(r); dbus_error_free(&e);
    return (int)has;
}

static const char *sentinel_call(DBusConnection *c, const char *user,
                                  const char *ssh_client, const char *ssh_tty)
{
    static char buf[64];
    DBusMessage *m = dbus_message_new_method_call(
        SENTINEL_BUS, SENTINEL_PATH, SENTINEL_IFACE, "Authenticate");
    if (!m) return NULL;

    DBusMessageIter it, arr;
    dbus_message_iter_init_append(m, &it);

    /* arg1: username (s) */
    if (!dbus_message_iter_append_basic(&it, DBUS_TYPE_STRING, &user))
        goto fail;

    /* arg2: session_env (a{ss}) */
    if (!dbus_message_iter_open_container(&it, DBUS_TYPE_ARRAY, "{ss}", &arr))
        goto fail;

    if (ssh_client && ssh_client[0]) {
        DBusMessageIter de; const char *k = "SSH_CLIENT";
        if (dbus_message_iter_open_container(&arr, DBUS_TYPE_DICT_ENTRY, NULL, &de)) {
            dbus_message_iter_append_basic(&de, DBUS_TYPE_STRING, &k);
            dbus_message_iter_append_basic(&de, DBUS_TYPE_STRING, &ssh_client);
            dbus_message_iter_close_container(&arr, &de);
        }
    }
    if (ssh_tty && ssh_tty[0]) {
        DBusMessageIter de; const char *k = "SSH_TTY";
        if (dbus_message_iter_open_container(&arr, DBUS_TYPE_DICT_ENTRY, NULL, &de)) {
            dbus_message_iter_append_basic(&de, DBUS_TYPE_STRING, &k);
            dbus_message_iter_append_basic(&de, DBUS_TYPE_STRING, &ssh_tty);
            dbus_message_iter_close_container(&arr, &de);
        }
    }
    dbus_message_iter_close_container(&it, &arr);

    DBusError e; dbus_error_init(&e);
    DBusMessage *r = dbus_connection_send_with_reply_and_block(
                         c, m, SENTINEL_DBUS_TIMEOUT_MS, &e);
    dbus_message_unref(m);
    if (dbus_error_is_set(&e) || !r) { dbus_error_free(&e); return NULL; }

    const char *res = NULL;
    DBusMessageIter out;
    dbus_message_iter_init(r, &out);
    if (dbus_message_iter_get_arg_type(&out) == DBUS_TYPE_STRING) {
        dbus_message_iter_get_basic(&out, &res);
        if (res) {
            strncpy(buf, res, sizeof(buf) - 1);
            buf[sizeof(buf) - 1] = '\0';
            res = buf;
        }
    }
    dbus_message_unref(r); dbus_error_free(&e);
    return res;

fail:
    dbus_message_unref(m); return NULL;
}

PAM_EXTERN int pam_sm_authenticate(pam_handle_t *pamh, int flags,
                                    int argc, const char **argv)
{
    (void)flags; (void)argc; (void)argv;

    /* Fail-Safe 1: If password was already supplied (e.g., entered in lock
     * screen password field or supplied by earlier module), do not intercept
     * or prompt. Step aside immediately so standard auth completes. */
    const void *authtok = NULL;
    if (pam_get_item(pamh, PAM_AUTHTOK, &authtok) == PAM_SUCCESS && authtok != NULL) {
        return PAM_IGNORE;
    }

    /* 0. Consent / attention gate — biometric presence is not consent.
     * Demand one interactive round-trip (an Enter press) before the camera
     * is engaged, so every grant is bound to a human who saw that an
     * authentication event was requested for the invoking context.
     * Non-interactive callers (sudo -n, cron, background daemons) cannot
     * answer a prompt: their conversation fails and we step aside
     * (PAM_IGNORE), letting the stack fall through to password auth.
     *
     * Fail-Safe 2: If the user typed their password into this prompt instead
     * of pressing Enter, we preserve it in PAM_AUTHTOK and return PAM_IGNORE
     * so pam_unix can authenticate it immediately without requiring re-entry. */
    const struct pam_conv *conv = NULL;
    if (pam_get_item(pamh, PAM_CONV, (const void **)&conv) != PAM_SUCCESS
        || conv == NULL || conv->conv == NULL)
        return PAM_IGNORE;
    struct pam_message cmsg;
    memset(&cmsg, 0, sizeof(cmsg));
    cmsg.msg_style = PAM_PROMPT_ECHO_OFF;
    cmsg.msg = "Sentinel face authentication requested - press Enter to scan (or enter password): ";
    const struct pam_message *cmsgp = &cmsg;
    struct pam_response *cresp = NULL;
    int crc = conv->conv(1, &cmsgp, &cresp, conv->appdata_ptr);
    if (crc != PAM_SUCCESS) {
        if (cresp) { free(cresp->resp); free(cresp); }
        return PAM_IGNORE; /* non-interactive caller or cancelled: never lock out */
    }
    if (cresp && cresp->resp && cresp->resp[0] != '\0') {
        /* User typed a password rather than empty Enter — pass it forward to pam_unix */
        pam_set_item(pamh, PAM_AUTHTOK, cresp->resp);
        free(cresp->resp); free(cresp);
        return PAM_IGNORE;
    }
    if (cresp) { free(cresp->resp); free(cresp); }

    /* 1. Get username — use exactly what PAM (greetd) reports; do NOT
     *    override with getuid() which returns root when greetd calls us. */
    const char *user = NULL;
    if (pam_get_user(pamh, &user, NULL) != PAM_SUCCESS || !user || user[0] == '\0')
        return PAM_IGNORE;

    /* 2. Connect to system DBus */
    DBusError e; dbus_error_init(&e);
    DBusConnection *conn = dbus_bus_get_private(DBUS_BUS_SYSTEM, &e);
    if (dbus_error_is_set(&e) || !conn) { dbus_error_free(&e); return PAM_IGNORE; }
    dbus_connection_set_exit_on_disconnect(conn, FALSE);

    /* 3. Check sentinel bus name is registered */
    if (!sentinel_reachable(conn)) {
        dbus_connection_close(conn); dbus_connection_unref(conn);
        return PAM_IGNORE;
    }

    /* 4. Collect SSH context from the *process* environment: the PAM env
     * is only written by pam_putenv(), which no real caller performs. */
    const char *ssh_c = getenv("SSH_CLIENT");
    const char *ssh_t = getenv("SSH_TTY");

    /* 5. Call Authenticate(username, session_env) */
    const char *result = sentinel_call(conn, user, ssh_c, ssh_t);
    dbus_connection_close(conn); dbus_connection_unref(conn);

    /* 6. DBus RPC failed / daemon returned no payload → transparent fallback.
     *    This is an infrastructure failure, not a recognition decision, so we
     *    step aside and let pam_unix.so prompt for a password silently. */
    if (!result) return PAM_IGNORE;

    /* 7. Map daemon result string → PAM return code.
     *
     * FAIL-SAFE GUARANTEE:
     * Only an explicit "GRANTED" recognition result satisfies PAM with PAM_SUCCESS.
     * Every other outcome ("NO_FACE", "TIMEOUT", "DENIED", "SPOOF", "REQUIRE_2FA",
     * or any unknown status/error) returns PAM_IGNORE.
     *
     * Why PAM_IGNORE for all non-granted cases?
     * Because returning PAM_AUTH_ERR can cause pam_faillock / pam_tally2 to record
     * failed attempts and lock out user accounts from password login. Returning
     * PAM_IGNORE guarantees that the PAM stack safely, transparently falls through
     * to standard password authentication (pam_unix.so). Biometric failures can
     * NEVER lock a user out of their own machine.
     */
    if (!strcmp(result, "GRANTED"))
        return PAM_SUCCESS;

    return PAM_IGNORE;
}

/* pam_sm_setcred: required export for PAM_SM_AUTH modules.
 * Sentinel does not manage credentials; return success to satisfy PAM. */
PAM_EXTERN int pam_sm_setcred(pam_handle_t *pamh, int flags,
                               int argc, const char **argv)
{
    (void)pamh; (void)flags; (void)argc; (void)argv;
    return PAM_SUCCESS;
}

/* pam_sentinel.c — Sentinel Recreated PAM bridge.
 * Zero biometric logic. Calls com.sentinel.Sentinel.Authenticate via libdbus-1.
 * Spec: docs/PAM_INTEGRATION.md  Constraint: < 200 lines C99. */

#include <string.h>
#include <unistd.h>
#include <sys/types.h>
#define PAM_SM_AUTH
#include <security/pam_modules.h>
#include <dbus/dbus.h>

#define SENTINEL_BUS  "com.sentinel.Sentinel"
#define SENTINEL_PATH "/com/sentinel/Sentinel"
#define SENTINEL_IFACE "com.sentinel.Sentinel"
#define SENTINEL_DBUS_TIMEOUT_MS 8000

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

    /* 4. Collect SSH env context */
    const char *ssh_c = pam_getenv(pamh, "SSH_CLIENT");
    const char *ssh_t = pam_getenv(pamh, "SSH_TTY");

    /* 5. Call Authenticate(username, session_env) */
    const char *result = sentinel_call(conn, user, ssh_c, ssh_t);
    dbus_connection_close(conn); dbus_connection_unref(conn);

    /* 6. DBus RPC failed / daemon returned no payload → transparent fallback.
     *    This is an infrastructure failure, not a recognition decision, so we
     *    step aside and let pam_unix.so prompt for a password silently. */
    if (!result) return PAM_IGNORE;

    /* 7. Map daemon result string → PAM return code.
     *
     * Semantic distinction:
     *   PAM_IGNORE   = "I have no opinion" — infrastructure not available or
     *                  no face was ever presented.  PAM silently falls through
     *                  to the next module (pam_unix.so → password prompt).
     *   PAM_AUTH_ERR = "I tried and it failed" — the daemon actively attempted
     *                  recognition but could not grant access.  PAM shows an
     *                  "authentication failed" notice before the password prompt,
     *                  giving the user clear feedback that face auth was attempted.
     *
     * Rule of thumb: if a camera session was opened and a face was involved,
     * use PAM_AUTH_ERR.  If the camera never meaningfully engaged, use PAM_IGNORE.
     */
    if (!strcmp(result, "GRANTED"))
        /* Face matched — unlock immediately. */
        return PAM_SUCCESS;

    if (!strcmp(result, "NO_FACE"))
        /* Daemon found no face in the field of view (camera opened but no
         * subject detected, or user walked away before detection).  No
         * recognition was attempted — silently fall through to password. */
        return PAM_IGNORE;

    if (!strcmp(result, "TIMEOUT"))
        /* Camera was open and a liveness session ran, but the user did not
         * complete the challenge within the time limit.  This is an active
         * failure: return PAM_AUTH_ERR so the lock screen shows a failure
         * notice before falling through to the password prompt.  This is the
         * correct UX — the user sees that face auth was attempted and expired,
         * then gets a clean password prompt rather than a silent transition. */
        return PAM_AUTH_ERR;

    if (!strcmp(result, "DENIED"))
        /* Face was detected and recognised but distance exceeded all thresholds.
         * Active failure — password prompt with failure notice. */
        return PAM_AUTH_ERR;

    if (!strcmp(result, "SPOOF"))
        /* Anti-spoof classifier rejected the presentation.  Active security
         * failure — password prompt with failure notice. */
        return PAM_AUTH_ERR;

    if (!strcmp(result, "REQUIRE_2FA"))
        /* Biometric passed at Tier 3 but policy requires a second factor.
         * Fall through to pam_unix.so so the user can supply their password
         * as the second factor.  PAM_AUTH_ERR triggers the prompt correctly. */
        return PAM_AUTH_ERR;

    /* Unknown / future result token or malformed payload.
     * Treat as infrastructure uncertainty — transparent fallback. */
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

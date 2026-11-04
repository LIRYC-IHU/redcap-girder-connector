#!/usr/bin/env bash
#
# Bring the running rgc-e2e stack to a ready state. Idempotent: every step checks what
# exists before creating it, so it can be re-run at any time (after a module VERSION bump,
# after `./run.sh up --build`, ...).
#
#   Girder : admin user, filesystem assetstore, non-admin user `redcap-module` with an API
#            key, collection "REDCap E2E" writable by that user.
#   REDCap : fresh install (install.php?auto=1, auth "None (Public)" => user site_admin),
#            super API token, module enabled system-wide, test project created through the
#            API, @GIRDER_UPLOAD instrument, module enabled + configured on the project.
#
# Generated test credentials and ids go to .state/state.json (gitignored). They are local
# throwaway values; nothing here is a real credential.
set -euo pipefail

E2E_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$E2E_DIR/../.." && pwd)"
STATE_DIR="$E2E_DIR/.state"
STATE_FILE="$STATE_DIR/state.json"

export MODULE_VERSION="${MODULE_VERSION:-$(tr -d '[:space:]' < "$REPO_DIR/VERSION")}"
MODULE_PREFIX="girder_uploader"

REDCAP_URL="http://localhost:18081"
GIRDER_URL="http://localhost:18080"
GIRDER_API="$GIRDER_URL/api/v1"

PROJECT_TITLE="Girder Uploader E2E"
INSTRUMENT="uploads"
UPLOAD_FIELD="girder_files"
# A second upload field: one field holds one upload, so a record's later
# examinations go in another one.
FOLLOW_UP_FIELD="girder_files_follow_up"
COLLECTION_NAME="REDCap E2E"
MODULE_USER="redcap-module"
API_KEY_NAME="redcap-e2e"

log() { printf '[provision] %s\n' "$*"; }
die() { printf '[provision] ERROR: %s\n' "$*" >&2; exit 1; }

compose() { docker compose -p rgc-e2e -f "$E2E_DIR/docker-compose.yml" "$@"; }

mkdir -p "$STATE_DIR"
chmod 700 "$STATE_DIR"
[ -f "$STATE_FILE" ] || echo '{}' > "$STATE_FILE"

state_get() { jq -r "$1 // empty" "$STATE_FILE"; }
state_set() { # state_set <jq path> <string value>
    local tmp; tmp="$(mktemp)"
    jq --arg v "$2" "$1 = \$v" "$STATE_FILE" > "$tmp" && mv "$tmp" "$STATE_FILE"
}
# Finite input: `tr < /dev/urandom | head` would die of SIGPIPE under pipefail.
random_secret() { head -c 512 /dev/urandom | LC_ALL=C tr -dc 'A-Za-z0-9' | cut -c "1-${1:-24}"; }
# Reuse a generated value from the state file, or generate and record it.
state_secret() { # state_secret <jq path> <length>
    local v; v="$(state_get "$1")"
    if [ -z "$v" ]; then v="$(random_secret "$2")"; state_set "$1" "$v"; fi
    printf '%s' "$v"
}

wait_for() { # wait_for <description> <url> [tries]
    local tries="${3:-90}" i=0
    until curl -fsS -o /dev/null "$2" 2>/dev/null; do
        i=$((i + 1)); [ "$i" -ge "$tries" ] && die "$1 not reachable at $2"
        sleep 2
    done
}

# --------------------------------------------------------------------------------------
# Girder
# --------------------------------------------------------------------------------------
provision_girder() {
    log "waiting for Girder ($GIRDER_API)"
    wait_for Girder "$GIRDER_API/system/version"

    local admin_login="admin" admin_pass module_pass
    admin_pass="$(state_secret '.girder.adminPassword' 24)"
    module_pass="$(state_secret '.girder.moduleUserPassword' 24)"
    state_set '.girder.apiUrl' "$GIRDER_API"
    state_set '.girder.adminLogin' "$admin_login"
    state_set '.girder.moduleUserLogin' "$MODULE_USER"

    # The first user created on an empty database becomes site administrator.
    curl -s -X POST "$GIRDER_API/user" \
        --data-urlencode "login=$admin_login" --data-urlencode "password=$admin_pass" \
        --data-urlencode "email=admin@e2e.test" --data-urlencode "firstName=E2E" \
        --data-urlencode "lastName=Admin" >/dev/null || true

    local token
    token="$(curl -s -u "$admin_login:$admin_pass" "$GIRDER_API/user/authentication" | jq -r '.authToken.token // empty')"
    [ -n "$token" ] || die "cannot log in as Girder admin. The volumes predate .state/: run ./run.sh reset"
    local H="Girder-Token: $token"
    [ "$(curl -s -H "$H" "$GIRDER_API/user/me" | jq -r '.admin')" = "true" ] \
        || die "Girder user '$admin_login' is not an administrator"

    # Filesystem assetstore.
    local assetstore_id
    assetstore_id="$(curl -s -H "$H" "$GIRDER_API/assetstore" | jq -r '.[0]._id // empty')"
    if [ -z "$assetstore_id" ]; then
        log "creating filesystem assetstore"
        assetstore_id="$(curl -s -H "$H" -X POST "$GIRDER_API/assetstore" \
            --data-urlencode "name=e2e-filesystem" --data-urlencode "type=0" \
            --data-urlencode "root=/var/girder/assetstore" | jq -r '._id // empty')"
        [ -n "$assetstore_id" ] || die "assetstore creation failed"
    fi
    state_set '.girder.assetstoreId' "$assetstore_id"

    # Non-admin user used by the module (the design forbids an admin account there).
    local module_user_id
    module_user_id="$(curl -s -H "$H" "$GIRDER_API/user?text=$MODULE_USER&limit=50" \
        | jq -r --arg l "$MODULE_USER" '.[] | select(.login == $l) | ._id' | head -1)"
    if [ -z "$module_user_id" ]; then
        log "creating Girder user $MODULE_USER"
        module_user_id="$(curl -s -H "$H" -X POST "$GIRDER_API/user" \
            --data-urlencode "login=$MODULE_USER" --data-urlencode "password=$module_pass" \
            --data-urlencode "email=redcap-module@e2e.test" --data-urlencode "firstName=REDCap" \
            --data-urlencode "lastName=Module" --data-urlencode "admin=false" | jq -r '._id // empty')"
        [ -n "$module_user_id" ] || die "creation of Girder user $MODULE_USER failed"
    fi
    state_set '.girder.moduleUserId' "$module_user_id"

    local mtoken
    mtoken="$(curl -s -u "$MODULE_USER:$module_pass" "$GIRDER_API/user/authentication" | jq -r '.authToken.token // empty')"
    [ -n "$mtoken" ] || die "cannot log in as $MODULE_USER"
    local MH="Girder-Token: $mtoken"
    [ "$(curl -s -H "$MH" "$GIRDER_API/user/me" | jq -r '.admin')" = "false" ] \
        || die "$MODULE_USER must not be a Girder administrator"

    # API key, full scope (scope omitted = full access).
    local api_key
    api_key="$(curl -s -H "$MH" "$GIRDER_API/api_key?userId=$module_user_id&limit=50" \
        | jq -r --arg n "$API_KEY_NAME" '.[] | select(.name == $n and .active == true) | .key' | head -1)"
    if [ -z "$api_key" ]; then
        log "creating API key for $MODULE_USER"
        api_key="$(curl -s -H "$MH" -X POST "$GIRDER_API/api_key" \
            --data-urlencode "name=$API_KEY_NAME" --data-urlencode "active=true" | jq -r '.key // empty')"
        [ -n "$api_key" ] || die "API key creation failed"
    fi
    state_set '.girder.apiKey' "$api_key"

    # Root collection, owned by the admin, WRITE for the module user.
    local collection_id
    collection_id="$(curl -s -H "$H" "$GIRDER_API/collection?text=$(jq -rn --arg s "$COLLECTION_NAME" '$s|@uri')&limit=50" \
        | jq -r --arg n "$COLLECTION_NAME" '.[] | select(.name == $n) | ._id' | head -1)"
    if [ -z "$collection_id" ]; then
        log "creating collection '$COLLECTION_NAME'"
        collection_id="$(curl -s -H "$H" -X POST "$GIRDER_API/collection" \
            --data-urlencode "name=$COLLECTION_NAME" --data-urlencode "public=false" | jq -r '._id // empty')"
        [ -n "$collection_id" ] || die "collection creation failed"
    fi
    local access
    access="$(jq -cn --arg u "$module_user_id" '{users: [{id: $u, level: 1}], groups: []}')"
    curl -s -H "$H" -X PUT "$GIRDER_API/collection/$collection_id/access" \
        --data-urlencode "access=$access" --data-urlencode "public=false" >/dev/null
    state_set '.girder.collectionId' "$collection_id"
    state_set '.girder.collectionName' "$COLLECTION_NAME"

    log "Girder ready: collection $collection_id"

    curl -s -H "$H" -X DELETE "$GIRDER_API/token/session" >/dev/null || true
    curl -s -H "$MH" -X DELETE "$GIRDER_API/token/session" >/dev/null || true
}

# --------------------------------------------------------------------------------------
# REDCap
# --------------------------------------------------------------------------------------
sql() { # sql <statement(s)>: tab-separated rows, no header
    compose exec -T mariadb mariadb -uroot -proot-e2e redcap -N -B -e "$1"
}
sql_quote() { printf "'%s'" "$(printf '%s' "$1" | sed "s/\\\\/\\\\\\\\/g; s/'/''/g")"; }

# Upsert an External Module setting (project_id NULL = system setting), stored exactly as
# the EM framework stores it (see ExternalModules::setSetting): booleans as 'true'/'false'.
em_setting() { # em_setting <module_id> <project_id|NULL> <key> <type> <value>
    local pid_cond="project_id = $2"; [ "$2" = "NULL" ] && pid_cond="project_id IS NULL"
    sql "DELETE FROM redcap_external_module_settings WHERE external_module_id = $1 AND $pid_cond AND \`key\` = $(sql_quote "$3");
         INSERT INTO redcap_external_module_settings (external_module_id, project_id, \`key\`, type, value)
         VALUES ($1, $2, $(sql_quote "$3"), $(sql_quote "$4"), $(sql_quote "$5"));"
}

redcap_api() { # redcap_api <token> <curl -d args...>
    local token="$1"; shift
    curl -sS -X POST "$REDCAP_URL/api/" --data-urlencode "token=$token" "$@"
}

provision_redcap() {
    log "waiting for REDCap ($REDCAP_URL)"
    wait_for REDCap "$REDCAP_URL/robots.txt"

    # 1. Fresh install through REDCap's own installer (auto mode = the installer's
    #    defaults, i.e. authentication "None (Public)", demo projects, API enabled).
    if ! sql "SELECT value FROM redcap_config WHERE field_name = 'redcap_version'" >/dev/null 2>&1; then
        log "installing REDCap (install.php?auto=1)"
        local out
        out="$(curl -sS --max-time 600 "$REDCAP_URL/install.php?auto=1")"
        case "$out" in
            *"Install completed"*) log "REDCap installed" ;;
            *) die "REDCap install did not complete: $(printf '%s' "$out" | head -c 500)" ;;
        esac
    fi
    local redcap_version
    redcap_version="$(sql "SELECT value FROM redcap_config WHERE field_name = 'redcap_version'")"
    [ -n "$redcap_version" ] || die "REDCap version not found in redcap_config"

    # 2. Site configuration for this stack.
    sql "UPDATE redcap_config SET value = '$REDCAP_URL/' WHERE field_name = 'redcap_base_url';
         UPDATE redcap_config SET value = 'none' WHERE field_name = 'auth_meth_global';
         UPDATE redcap_config SET value = '0' WHERE field_name = 'auto_report_stats';
         UPDATE redcap_config SET value = '1' WHERE field_name = 'api_enabled';"
    [ "$(sql "SELECT super_user FROM redcap_user_information WHERE username = 'site_admin'")" = "1" ] \
        || die "site_admin is missing or not a super user"

    # 3. Super API token for site_admin (64 hex chars), used only to create the project.
    local super_token
    super_token="$(state_get '.redcap.superToken')"
    if [ -z "$super_token" ]; then
        super_token="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' | tr 'a-f' 'A-F')"
        state_set '.redcap.superToken' "$super_token"
    fi
    sql "UPDATE redcap_user_information SET api_token = '$super_token' WHERE username = 'site_admin'"

    # 4. Module enabled system-wide at the mounted version.
    compose exec -T redcap test -f "/var/www/html/modules/${MODULE_PREFIX}_v${MODULE_VERSION}/config.json" \
        || die "module not mounted at modules/${MODULE_PREFIX}_v${MODULE_VERSION} (VERSION changed? run ./run.sh up)"
    sql "INSERT IGNORE INTO redcap_external_modules (directory_prefix) VALUES ('$MODULE_PREFIX')"
    local module_id
    module_id="$(sql "SELECT external_module_id FROM redcap_external_modules WHERE directory_prefix = '$MODULE_PREFIX'")"
    # The EM framework stores the version as the directory suffix: v<VERSION>.
    em_setting "$module_id" NULL version string "v$MODULE_VERSION"

    # 5. Test project, created through the API so REDCap's own code builds the arms,
    #    events and creator rights; the data dictionary is then imported the same way.
    local pid
    pid="$(sql "SELECT project_id FROM redcap_projects WHERE app_title = '$PROJECT_TITLE' AND date_deleted IS NULL ORDER BY project_id LIMIT 1")"
    if [ -z "$pid" ]; then
        log "creating REDCap project '$PROJECT_TITLE'"
        local data out
        data="$(jq -cn --arg t "$PROJECT_TITLE" '[{project_title: $t, purpose: 0, is_longitudinal: 0, surveys_enabled: 0, record_autonumbering_enabled: 0}]')"
        out="$(redcap_api "$super_token" -d content=project -d format=json --data-urlencode "data=$data")"
        [ "${#out}" -eq 32 ] || die "project creation failed: $out"
        pid="$(sql "SELECT project_id FROM redcap_user_rights WHERE api_token = '$out'")"
        [ -n "$pid" ] || die "project created but its id was not found"
    fi
    local project_token
    project_token="$(sql "SELECT api_token FROM redcap_user_rights WHERE project_id = $pid AND username = 'site_admin'")"
    [ -n "$project_token" ] && [ "$project_token" != "NULL" ] || die "no API token for site_admin on project $pid"

    local metadata out
    # REDCap reads JSON metadata positionally: every column, in data-dictionary order.
    metadata="$(jq -cn --arg form "$INSTRUMENT" --arg field "$UPLOAD_FIELD" --arg followup "$FOLLOW_UP_FIELD" '
      def row($name; $type; $label; $annotation): {
        field_name: $name, form_name: $form, section_header: "", field_type: $type,
        field_label: $label, select_choices_or_calculations: "", field_note: "",
        text_validation_type_or_show_slider_number: "", text_validation_min: "",
        text_validation_max: "", identifier: "", branching_logic: "", required_field: "",
        custom_alignment: "", question_number: "", matrix_group_name: "", matrix_ranking: "",
        field_annotation: $annotation };
      [ row("record_id"; "text"; "Record ID"; ""),
        row($field; "notes"; "Files (Girder)"; "@GIRDER_UPLOAD"),
        row($followup; "notes"; "Follow-up files (Girder)"; "@GIRDER_UPLOAD") ]')"
    out="$(redcap_api "$project_token" -d content=metadata -d format=json --data-urlencode "data=$metadata")"
    [ "$out" = "3" ] || die "data dictionary import failed: $out"

    local event_id
    event_id="$(sql "SELECT m.event_id FROM redcap_events_metadata m JOIN redcap_events_arms a ON a.arm_id = m.arm_id
                     WHERE a.project_id = $pid ORDER BY m.event_id LIMIT 1")"
    [ -n "$event_id" ] || die "no event found for project $pid"

    # REDCap quirk: while a project's record-list cache is NOT_STARTED, the first request
    # into the project is redirected to DataEntryController:buildRecordListCache. When that
    # first request is the module's AJAX POST, it receives a 302 to an HTML page and the
    # upload fails ("Unexpected token '<'"). Build the cache now.
    curl -sS -L -o /dev/null --max-time 120 \
        "$REDCAP_URL/redcap_v$redcap_version/index.php?pid=$pid&route=DataEntryController:buildRecordListCache"
    local cache_status
    cache_status="$(sql "SELECT record_list_status FROM redcap_record_counts WHERE project_id = $pid")"
    [ "$cache_status" = "COMPLETE" ] || log "WARNING: record list cache status is '$cache_status'"

    # 6. Module enabled + configured on the project.
    local api_key collection_id
    api_key="$(state_get '.girder.apiKey')"
    collection_id="$(state_get '.girder.collectionId')"
    [ -n "$api_key" ] && [ -n "$collection_id" ] || die "Girder state missing: provision Girder first"
    em_setting "$module_id" "$pid" enabled boolean true
    em_setting "$module_id" "$pid" girder-backchannel-api-url string "http://girder:8080/api/v1"
    em_setting "$module_id" "$pid" girder-frontchannel-base-url string "$GIRDER_URL"
    em_setting "$module_id" "$pid" api-key string "$api_key"
    em_setting "$module_id" "$pid" root-collection-id string "$collection_id"
    em_setting "$module_id" "$pid" deidentify-dicom boolean true
    em_setting "$module_id" "$pid" deidentify-xml-ecg boolean true
    em_setting "$module_id" "$pid" deidentify-schiller-holter boolean true
    em_setting "$module_id" "$pid" preserve-upload-folder-architecture boolean true

    state_set '.redcap.baseUrl' "$REDCAP_URL"
    state_set '.redcap.version' "$redcap_version"
    state_set '.redcap.username' "site_admin"
    state_set '.redcap.projectId' "$pid"
    state_set '.redcap.projectTitle' "$PROJECT_TITLE"
    state_set '.redcap.projectToken' "$project_token"
    state_set '.redcap.eventId' "$event_id"
    state_set '.redcap.instrument' "$INSTRUMENT"
    state_set '.redcap.uploadField' "$UPLOAD_FIELD"
    state_set '.redcap.followUpField' "$FOLLOW_UP_FIELD"
    state_set '.module.prefix' "$MODULE_PREFIX"
    state_set '.module.version' "$MODULE_VERSION"
    state_set '.module.externalModuleId' "$module_id"
    log "REDCap ready: v$redcap_version, project $pid, event $event_id, module ${MODULE_PREFIX}_v$MODULE_VERSION"
}

check_wasm() {
    if [ ! -f "$REPO_DIR/src/wasm/dicom_deid/pkg/dicom_deid_bg.wasm" ]; then
        log "WARNING: src/wasm/dicom_deid/pkg is missing; run scripts/build.sh before the tests"
    fi
}

provision_girder
provision_redcap
check_wasm
log "state written to $STATE_FILE"

// The module's External Module settings, read and written straight in REDCap's database,
// stored the way the EM framework stores them (see provision.sh, em_setting).
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { loadState, E2E_DIR } = require('./state');

function compose(args, options = {}) {
    return execFileSync(
        'docker',
        ['compose', '-p', 'rgc-e2e', '-f', path.join(E2E_DIR, 'docker-compose.yml'), ...args],
        { encoding: 'utf8', ...options }
    );
}

function sql(statement) {
    return compose(['exec', '-T', 'mariadb', 'mariadb', '-uroot', '-proot-e2e', 'redcap', '-N', '-B', '-e', statement]).trim();
}

function quote(value) {
    return `'${String(value).replace(/\\/g, '\\\\').replace(/'/g, "''")}'`;
}

function scope(projectId) {
    const moduleId = Number(loadState().module.externalModuleId);
    const project = projectId === null ? 'project_id IS NULL' : `project_id = ${Number(projectId)}`;
    return `external_module_id = ${moduleId} AND ${project}`;
}

function projectId() {
    return Number(loadState().redcap.projectId);
}

/** A project setting's raw value, or null when absent. */
function getProjectSetting(key) {
    const value = sql(`SELECT value FROM redcap_external_module_settings WHERE ${scope(projectId())} AND \`key\` = ${quote(key)}`);
    return value === '' ? null : value;
}

function setSetting(pid, key, type, value) {
    const moduleId = Number(loadState().module.externalModuleId);
    sql(`DELETE FROM redcap_external_module_settings WHERE ${scope(pid)} AND \`key\` = ${quote(key)};
         INSERT INTO redcap_external_module_settings (external_module_id, project_id, \`key\`, type, value)
         VALUES (${moduleId}, ${pid === null ? 'NULL' : Number(pid)}, ${quote(key)}, ${quote(type)}, ${quote(value)});`);
}

function removeSetting(pid, key) {
    sql(`DELETE FROM redcap_external_module_settings WHERE ${scope(pid)} AND \`key\` = ${quote(key)}`);
}

/** The patient fingerprint REDCap stores for a record, or null. */
function storedFingerprint(recordId) {
    return getProjectSetting(`pseudonym-fingerprint:${recordId}`);
}

function setFingerprintCheck(enabled) {
    setSetting(projectId(), 'pseudonym-fingerprint-check', 'boolean', enabled ? 'true' : 'false');
}

/** REDCap's own daily limit (a system setting); null restores the default. */
function setDailyLimit(limit) {
    if (limit === null) {
        removeSetting(null, 'pseudonym-daily-limit');
    } else {
        setSetting(null, 'pseudonym-daily-limit', 'string', String(limit));
    }
}

/** Evaluations the module logged for the project over the last 24 hours. */
function recentEvaluations() {
    const moduleId = Number(loadState().module.externalModuleId);
    return Number(sql(`SELECT COUNT(*) FROM redcap_external_modules_log
        WHERE external_module_id = ${moduleId} AND project_id = ${projectId()}
        AND message = 'pseudonym-evaluate' AND timestamp > NOW() - INTERVAL 1 DAY`) || 0);
}

/** How many times the module logged `message` for the project. */
function countLogs(message) {
    const moduleId = Number(loadState().module.externalModuleId);
    return Number(sql(`SELECT COUNT(*) FROM redcap_external_modules_log
        WHERE external_module_id = ${moduleId} AND project_id = ${projectId()}
        AND message = ${quote(message)}`) || 0);
}

/** How many entries of the project's Logging page carry this description. */
function countProjectLogs(description) {
    const table = sql(`SELECT log_event_table FROM redcap_projects WHERE project_id = ${projectId()}`);
    return Number(sql(`SELECT COUNT(*) FROM ${table}
        WHERE project_id = ${projectId()} AND description = ${quote(description)}`) || 0);
}

/** Forget the fingerprints of records whose id starts with `prefix`. */
function forgetFingerprints(prefix) {
    sql(`DELETE FROM redcap_external_module_settings WHERE ${scope(projectId())}
        AND \`key\` LIKE ${quote(`pseudonym-fingerprint:${prefix}%`)}`);
}

/** The Girder folder of the project's event: `{event_id} ({event_name})`. */
function eventFolderName() {
    const { eventId } = loadState().redcap;
    return `${eventId} (${sql(`SELECT descrip FROM redcap_events_metadata WHERE event_id = ${Number(eventId)}`)})`;
}

/** A project user whose right on the instrument is `formRight` (129 read-only, 130 view & edit). */
function ensureProjectUser(username, formRight) {
    const { instrument } = loadState().redcap;
    sql(`INSERT IGNORE INTO redcap_user_information (username, user_email, user_firstname, user_lastname)
         VALUES (${quote(username)}, ${quote(`${username}@e2e.test`)}, 'E2E', ${quote(username)});
         DELETE FROM redcap_user_rights WHERE project_id = ${projectId()} AND username = ${quote(username)};
         INSERT INTO redcap_user_rights (project_id, username, data_entry)
         VALUES (${projectId()}, ${quote(username)}, ${quote(`[${instrument},${formRight}]`)});`);
}

/** A system setting's raw value, or null when absent. */
function getSystemSetting(key) {
    const value = sql(`SELECT value FROM redcap_external_module_settings WHERE ${scope(null)} AND \`key\` = ${quote(key)}`);
    return value === '' ? null : value;
}

/** Put a released version's files next to the current one in REDCap's modules folder. */
function installVersion(version, sourceDir) {
    const target = `/var/www/html/modules/${loadState().module.prefix}_${version}`;
    compose(['exec', '-T', 'redcap', 'mkdir', '-p', target]);
    compose(['cp', `${sourceDir}/.`, `redcap:${target}`]);
    compose(['exec', '-T', 'redcap', 'chmod', '-R', 'a+rX', target]);
}

function removeVersion(version) {
    compose(['exec', '-T', 'redcap', 'rm', '-rf', `/var/www/html/modules/${loadState().module.prefix}_${version}`]);
}

/**
 * Make the database look as a 1.x installation left it: that version enabled, and
 * nothing of the pseudonyms anywhere. The project's other settings stay.
 */
function rollBackTo(version) {
    setSetting(null, 'version', 'string', version);
    sql(`DELETE FROM redcap_external_module_settings WHERE external_module_id = ${Number(loadState().module.externalModuleId)}
        AND \`key\` LIKE 'pseudonym-%'`);
}

module.exports = {
    countLogs,
    countProjectLogs,
    getSystemSetting,
    eventFolderName,
    ensureProjectUser,
    removeSetting,
    installVersion,
    removeVersion,
    rollBackTo,
    setSetting,
    forgetFingerprints,
    compose,
    sql,
    getProjectSetting,
    storedFingerprint,
    setFingerprintCheck,
    setDailyLimit,
    recentEvaluations,
};

<?php

namespace ExternalModules;

/**
 * Minimal stand-in for the REDCap External Module base class.
 *
 * The real class lives inside a REDCap installation and pulls in the whole
 * framework; the tests only need project settings and the handful of helpers
 * the module calls, so the module can be loaded outside REDCap.
 */
class AbstractExternalModule
{
    /** @var array<string, mixed> */
    public $projectSettings = [];

    /**
     * What the framework's Project::getRights() returns, by username: the raw
     * redcap_user_rights row (null for a user who is not on the project).
     *
     * @var array<string, array<string, mixed>|null>|null null: no project context
     */
    public $projectRights = null;

    /** @var array<int, string> */
    public $superUsers = [];

    public function getProjectSetting($key, $pid = null)
    {
        return array_key_exists($key, $this->projectSettings) ? $this->projectSettings[$key] : null;
    }

    public function setProjectSetting($key, $value, $pid = null)
    {
        $this->projectSettings[$key] = $value;
    }

    public function removeProjectSetting($key, $pid = null)
    {
        unset($this->projectSettings[$key]);
    }

    /** @var array<string, mixed> */
    public $systemSettings = [];

    public function getSystemSetting($key)
    {
        return array_key_exists($key, $this->systemSettings) ? $this->systemSettings[$key] : null;
    }

    public function setSystemSetting($key, $value)
    {
        $this->systemSettings[$key] = $value;
    }

    /** @var array<int, array{0: string, 1: array}> Messages logged, in order. */
    public $logs = [];

    /**
     * Only the count the rate limit asks for: `select count(1) as evaluations
     * where message = ? ...`, answered from the messages logged so far.
     */
    public function queryLogs($sql, $parameters = [])
    {
        $message = $parameters[0] ?? null;
        $count = count(array_filter($this->logs, function ($entry) use ($message) {
            return $entry[0] === $message;
        }));
        return new class ($count) {
            private $count;
            public function __construct($count) { $this->count = $count; }
            public function fetch_assoc() { return ['evaluations' => $this->count]; }
        };
    }

    /** Database locks always succeed: the tests run in one process. */
    public function query($sql, $parameters = [])
    {
        return new class {
            public function fetch_assoc() { return ['acquired' => 1, 'released' => 1]; }
        };
    }

    public function getUrl($path, $noAuth = false, $useApiEndpoint = false)
    {
        return 'https://redcap.example.org/modules/girder_uploader/' . ltrim((string) $path, './');
    }

    public function getJavascriptModuleObjectName()
    {
        return 'ExternalModules.GirderUploaderModule';
    }

    public function initializeJavascriptModuleObject()
    {
        return '';
    }

    public function getProject($projectId = null)
    {
        return $this->projectRights === null ? null : new StubProject($this->projectRights);
    }

    public function getUser($username = null)
    {
        return new StubUser(in_array($username, $this->superUsers, true));
    }

    public function log($message, $parameters = [])
    {
        $this->logs[] = [(string) $message, (array) $parameters];
    }
}

class StubProject
{
    /** @var array<string, array<string, mixed>|null> */
    private $rights;

    public function __construct(array $rights)
    {
        $this->rights = $rights;
    }

    public function getRights($username)
    {
        return isset($this->rights[$username]) ? $this->rights[$username] : null;
    }
}

class StubUser
{
    /** @var bool */
    private $superUser;

    public function __construct($superUser)
    {
        $this->superUser = (bool) $superUser;
    }

    public function isSuperUser()
    {
        return $this->superUser;
    }
}

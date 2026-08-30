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

    public function getProjectSetting($key, $pid = null)
    {
        return array_key_exists($key, $this->projectSettings) ? $this->projectSettings[$key] : null;
    }

    public function setProjectSetting($key, $value, $pid = null)
    {
        $this->projectSettings[$key] = $value;
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
        return null;
    }

    public function log($message, $parameters = [])
    {
    }
}

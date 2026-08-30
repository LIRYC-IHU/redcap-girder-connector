<?php

/**
 * Test runner for the REDCap module.
 *
 * REDCap external modules ship as plain PHP with no autoloader and no
 * dependencies, so the suite stays dependency-free too: `php tests/php/run.php`
 * is all it takes, on any PHP the module itself supports.
 */

declare(strict_types=1);

namespace GirderUploaderTests;

require_once __DIR__ . '/stubs/AbstractExternalModule.php';
require_once __DIR__ . '/../../src/GirderUploaderModule.php';

use ExternalModules\GirderUploaderModule\GirderUploaderModule;
use ReflectionMethod;

final class TestRunner
{
    /** @var array<int, array{0: string, 1: callable}> */
    private array $tests = [];
    private int $passed = 0;
    /** @var array<int, string> */
    private array $failures = [];
    private string $currentTest = '';

    public function test(string $name, callable $body): void
    {
        $this->tests[] = [$name, $body];
    }

    public function assertSame($expected, $actual, string $message = ''): void
    {
        if ($expected === $actual) {
            return;
        }

        $this->fail(sprintf(
            "%s\n    expected: %s\n    actual:   %s",
            $message !== '' ? $message : 'values differ',
            var_export($expected, true),
            var_export($actual, true)
        ));
    }

    public function assertTrue($actual, string $message = ''): void
    {
        $this->assertSame(true, $actual, $message !== '' ? $message : 'expected true');
    }

    public function assertFalse($actual, string $message = ''): void
    {
        $this->assertSame(false, $actual, $message !== '' ? $message : 'expected false');
    }

    private function fail(string $message): void
    {
        throw new \RuntimeException($message);
    }

    public function run(): int
    {
        foreach ($this->tests as [$name, $body]) {
            $this->currentTest = $name;
            try {
                $body($this);
                $this->passed++;
                fwrite(STDOUT, "  ok  {$name}\n");
            } catch (\Throwable $error) {
                $this->failures[] = "{$name}\n    " . str_replace("\n", "\n  ", $error->getMessage());
                fwrite(STDOUT, "  FAIL {$name}\n");
            }
        }

        fwrite(STDOUT, sprintf(
            "\n%d passed, %d failed\n",
            $this->passed,
            count($this->failures)
        ));

        foreach ($this->failures as $failure) {
            fwrite(STDOUT, "\nFAILED: {$failure}\n");
        }

        return count($this->failures) === 0 ? 0 : 1;
    }
}

/** Call a private method of the module under test. */
function invoke(GirderUploaderModule $module, string $method, array $arguments = [])
{
    $reflection = new ReflectionMethod(GirderUploaderModule::class, $method);
    $reflection->setAccessible(true);

    return $reflection->invokeArgs($module, $arguments);
}

function moduleWithSettings(array $settings = []): GirderUploaderModule
{
    $module = new GirderUploaderModule();
    $module->projectSettings = $settings;

    return $module;
}

$runner = new TestRunner();

foreach (glob(__DIR__ . '/*.test.php') as $file) {
    (require $file)($runner);
}

exit($runner->run());

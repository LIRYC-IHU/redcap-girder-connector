<?php

/**
 * The server half of the pseudonym VOPRF. It must agree byte for byte with the
 * browser (crates/liryc-pseudonym), so it is pinned to RFC 9497's own vectors and
 * to the vectors of protocol/test-vectors/pseudonym.json.
 */

declare(strict_types=1);

require_once __DIR__ . '/../../src/Voprf.php';

use ExternalModules\GirderUploaderModule\Voprf;

return function ($t): void {
    $vectorDir = __DIR__ . '/../../protocol/test-vectors';
    $rfc = json_decode((string) file_get_contents($vectorDir . '/rfc9497-ristretto255-sha512-voprf.json'), true, 512, JSON_THROW_ON_ERROR);
    $vectors = json_decode((string) file_get_contents($vectorDir . '/pseudonym.json'), true, 512, JSON_THROW_ON_ERROR);

    // Group order L, little-endian: the smallest non-canonical scalar.
    $order = 'edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010';
    $zero = str_repeat('00', 32);
    // The ristretto255 generator, a valid element unrelated to the vectors.
    $generator = 'e2f2ae0a6abc4e71a884a961c500515f58e30b6aa582dd8db6a65945e08d2d76';

    $assertThrows = static function ($t, callable $body, string $message): void {
        try {
            $body();
        } catch (\InvalidArgumentException $expected) {
            return;
        }
        $t->assertSame('InvalidArgumentException', 'nothing thrown', $message);
    };

    /** Flip the lowest bit of byte $index of a hex string. */
    $flip = static function (string $hex, int $index): string {
        $bytes = hex2bin($hex);
        $bytes[$index] = chr(ord($bytes[$index]) ^ 0x01);

        return bin2hex($bytes);
    };

    $t->test('voprf: RFC 9497 A.1.2 public key', function ($t) use ($rfc): void {
        $t->assertSame($rfc['pkSm'], Voprf::publicKey($rfc['skSm']));
    });

    $t->test('voprf: RFC 9497 A.1.2 evaluation and proof are exact', function ($t) use ($rfc): void {
        foreach ($rfc['vectors'] as $i => $vector) {
            $result = Voprf::blindEvaluate($rfc['skSm'], $vector['blindedElement'], $vector['proofRandomScalar']);

            $t->assertSame($vector['evaluationElement'], $result['evaluatedElement'], "vector {$i}: evaluated element");
            $t->assertSame($vector['proof'], $result['proof'], "vector {$i}: proof");
            $t->assertSame($rfc['pkSm'], $result['publicKey'], "vector {$i}: public key");
        }
    });

    $t->test('voprf: RFC 9497 A.1.2 proofs verify', function ($t) use ($rfc): void {
        foreach ($rfc['vectors'] as $i => $vector) {
            $t->assertTrue(
                Voprf::verifyProof($rfc['pkSm'], $vector['blindedElement'], $vector['evaluationElement'], $vector['proof']),
                "vector {$i}"
            );
        }
    });

    $t->test('voprf: protocol vectors', function ($t) use ($vectors): void {
        $result = Voprf::blindEvaluate($vectors['key'], $vectors['blindedElement'], $vectors['proofRandomScalar']);

        $t->assertSame($vectors['publicKey'], Voprf::publicKey($vectors['key']));
        $t->assertSame($vectors['evaluatedElement'], $result['evaluatedElement']);
        $t->assertSame($vectors['proof'], $result['proof']);
        $t->assertSame($vectors['publicKey'], $result['publicKey']);
        $t->assertTrue(Voprf::verifyProof($vectors['publicKey'], $vectors['blindedElement'], $result['evaluatedElement'], $result['proof']));
    });

    $t->test('voprf: proofs with fresh randomness verify and differ', function ($t) use ($vectors): void {
        $key = $vectors['key'];
        $first = Voprf::blindEvaluate($key, $vectors['blindedElement']);
        $second = Voprf::blindEvaluate($key, $vectors['blindedElement']);

        $t->assertSame($vectors['evaluatedElement'], $first['evaluatedElement']);
        $t->assertSame($first['evaluatedElement'], $second['evaluatedElement']);
        $t->assertFalse($first['proof'] === $second['proof'], 'proof randomness is fresh');
        foreach ([$first, $second] as $result) {
            $t->assertSame(1, preg_match('/\A[0-9a-f]{128}\z/', $result['proof']), 'proof is lowercase hex');
            $t->assertTrue(Voprf::verifyProof($result['publicKey'], $vectors['blindedElement'], $result['evaluatedElement'], $result['proof']));
        }

        $key = Voprf::generateKey();
        $result = Voprf::blindEvaluate($key, $vectors['blindedElement']);
        $t->assertTrue(Voprf::verifyProof(Voprf::publicKey($key), $vectors['blindedElement'], $result['evaluatedElement'], $result['proof']));
    });

    $t->test('voprf: a tampered proof, element or key fails verification', function ($t) use ($vectors, $flip, $generator, $order, $zero): void {
        $pk = $vectors['publicKey'];
        $otherPublicKey = Voprf::publicKey(Voprf::generateKey());
        $b = $vectors['blindedElement'];
        $z = $vectors['evaluatedElement'];
        $proof = $vectors['proof'];

        $t->assertFalse(Voprf::verifyProof($pk, $b, $z, $flip($proof, 0)), 'c altered');
        $t->assertFalse(Voprf::verifyProof($pk, $b, $z, $flip($proof, 32)), 's altered');
        $t->assertFalse(Voprf::verifyProof($pk, $b, $z, substr($proof, 64) . substr($proof, 0, 64)), 'c and s swapped');
        $t->assertFalse(Voprf::verifyProof($pk, $b, $z, $zero . $zero), 'zero proof');
        $t->assertFalse(Voprf::verifyProof($pk, $b, $z, substr($proof, 0, 64) . $zero), 'zero s');
        $t->assertFalse(Voprf::verifyProof($pk, $b, $z, substr($proof, 0, 64) . $order), 'non-canonical s');
        $t->assertFalse(Voprf::verifyProof($pk, $b, $z, substr($proof, 0, 126)), 'short proof');

        $t->assertFalse(Voprf::verifyProof($pk, $b, $generator, $proof), 'other evaluated element');
        $t->assertFalse(Voprf::verifyProof($pk, $generator, $z, $proof), 'other blinded element');
        $t->assertFalse(Voprf::verifyProof($pk, $b, $zero, $proof), 'identity evaluated element');
        $t->assertFalse(Voprf::verifyProof($pk, $z, $b, $proof), 'elements swapped');

        $t->assertFalse(Voprf::verifyProof($otherPublicKey, $b, $z, $proof), 'other public key');
        $t->assertFalse(Voprf::verifyProof($zero, $b, $z, $proof), 'identity public key');
        $t->assertFalse(Voprf::verifyProof(str_repeat('ff', 32), $b, $z, $proof), 'invalid public key');

        // A server evaluating with another key than the one it publishes.
        $impostor = Voprf::blindEvaluate(Voprf::generateKey(), $z);
        $t->assertFalse(Voprf::verifyProof($otherPublicKey, $z, $impostor['evaluatedElement'], $impostor['proof']), 'impostor key');
    });

    $t->test('voprf: an invalid blinded element is rejected', function ($t) use ($vectors, $assertThrows, $zero): void {
        $key = $vectors['key'];
        $b = $vectors['blindedElement'];

        $cases = [
            'identity' => $zero,
            'not on the group' => str_repeat('ff', 32),
            'non-canonical encoding' => 'edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f',
            'short' => substr($b, 0, 62),
            'long' => $b . '00',
            'not hex' => 'zz' . substr($b, 2),
            'empty' => '',
        ];
        foreach ($cases as $name => $element) {
            $assertThrows($t, static function () use ($key, $element): void {
                Voprf::blindEvaluate($key, $element);
            }, "blinded element: {$name}");
        }
    });

    $t->test('voprf: an invalid key is rejected', function ($t) use ($vectors, $assertThrows, $order, $zero): void {
        $cases = [
            'zero' => $zero,
            'group order' => $order,
            'all ones' => str_repeat('ff', 32),
            'short' => substr($vectors['key'], 0, 62),
            'not hex' => 'zz' . substr($vectors['key'], 2),
            'empty' => '',
        ];
        foreach ($cases as $name => $key) {
            $t->assertFalse(Voprf::isValidKey($key), "isValidKey: {$name}");
            $assertThrows($t, static function () use ($key, $vectors): void {
                Voprf::blindEvaluate($key, $vectors['blindedElement']);
            }, "blindEvaluate: {$name}");
            $assertThrows($t, static function () use ($key): void {
                Voprf::publicKey($key);
            }, "publicKey: {$name}");
        }

        $t->assertTrue(Voprf::isValidKey($vectors['key']));
        $t->assertFalse(Voprf::isValidKey(substr($order, 0, 63) . 'f'), 'above the order');
        $t->assertTrue(Voprf::isValidKey('ecd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010'), 'order minus one');
    });

    $t->test('voprf: zero or non-canonical proof randomness is rejected', function ($t) use ($vectors, $assertThrows, $order, $zero): void {
        foreach (['zero' => $zero, 'group order' => $order, 'short' => '01'] as $name => $random) {
            $assertThrows($t, static function () use ($vectors, $random): void {
                Voprf::blindEvaluate($vectors['key'], $vectors['blindedElement'], $random);
            }, "proof randomness: {$name}");
        }
    });

    $t->test('voprf: generated keys are valid and distinct', function ($t): void {
        $keys = [];
        for ($i = 0; $i < 32; $i++) {
            $key = Voprf::generateKey();
            $t->assertSame(1, preg_match('/\A[0-9a-f]{64}\z/', $key), 'lowercase hex');
            $t->assertTrue(Voprf::isValidKey($key));
            $t->assertSame(1, preg_match('/\A[0-9a-f]{64}\z/', Voprf::publicKey($key)), 'public key is lowercase hex');
            $keys[$key] = true;
        }
        $t->assertSame(32, count($keys));
    });

    $t->test('element decoding follows RFC 9496, not libsodium 1.0.18', function ($t): void {
        // Shared with the browser implementation. libsodium 1.0.18
        // accepts the identity and encodings with bit 255 set; the RFC does not.
        $fixture = json_decode((string) file_get_contents(__DIR__ . '/../../protocol/test-vectors/ristretto255-decoding.json'), true);
        $mismatches = [];
        foreach ($fixture['cases'] as $case) {
            try {
                $answer = Voprf::blindEvaluate($fixture['key'], $case['element']);
                $accepted = true;
            } catch (\InvalidArgumentException $error) {
                $accepted = false;
            }
            if ($accepted !== $case['valid']
                || ($accepted && $case['kTimesElement'] !== null && $answer['evaluatedElement'] !== $case['kTimesElement'])) {
                $mismatches[] = $case['element'];
            }
        }
        $t->assertSame([], $mismatches, count($fixture['cases']) . ' encodings checked');
    });
};

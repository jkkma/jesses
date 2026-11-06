import { expect, test } from '@playwright/test';
import { adjustedOutputTitle } from '../../src/lib/components/shared/output-title';

test('a resized SDR rendering does not keep the source resolution and HDR tokens', () => {
  const source = 'Resident.Evil.2026.2160p.iT.WEB-DL.DV.HDR10+[Ben The Men]';
  expect(adjustedOutputTitle(source, { toneMapped: true, height: 1080 })).toBe(
    'Resident.Evil.2026.1080p.iT.WEB-DL[Ben The Men]',
  );
  expect(adjustedOutputTitle(source, { toneMapped: false, height: 720 })).toBe(
    'Resident.Evil.2026.720p.iT.WEB-DL.DV.HDR10+[Ben The Men]',
  );
  expect(adjustedOutputTitle(source, { toneMapped: true })).toBe(
    'Resident.Evil.2026.2160p.iT.WEB-DL[Ben The Men]',
  );
  // A resize to an unknown height drops the stale resolution.
  expect(adjustedOutputTitle('Film 4K UHD HDR (2019)', { toneMapped: true, height: null })).toBe(
    'Film (2019)',
  );
  expect(adjustedOutputTitle('Movie [Dolby Vision]', { toneMapped: true })).toBe('Movie');
});

test('unchanged pictures and ordinary words keep the source title', () => {
  const title = 'DVD Extras - Driving Lessons 1080p';
  expect(adjustedOutputTitle(title, { toneMapped: false })).toBe(title);
  expect(adjustedOutputTitle(title, { toneMapped: true })).toBe(title);
  expect(adjustedOutputTitle('Hdrive.Story', { toneMapped: true })).toBe('Hdrive.Story');
  expect(adjustedOutputTitle('', { toneMapped: true, height: 1080 })).toBe('');
});

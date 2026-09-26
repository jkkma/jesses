import type { EncoderParameter, EncoderParameterCatalog } from '$lib/ipc/generated';
import { parameterError } from './encoder-parameters';

/** Parse argument data without interpreting a shell or expanding paths. */
export function commandTokens(text: string): string[] {
  if (text.length > 16_384 || /[\x00-\x08\x0b\x0c\x0e-\x1f]/.test(text))
    throw new Error('Use at most 16,384 characters without control characters.');
  const result: string[] = [];
  let token = '';
  let quote = '';
  let started = false;
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (char === '\\' && quote && text[i + 1] === quote) {
      token += text[++i];
      started = true;
    } else if (quote) {
      if (char === quote) quote = '';
      else token += char;
    } else if (char === '"' || char === "'") {
      quote = char;
      started = true;
    } else if (/\s/.test(char)) {
      if (started) result.push(token);
      token = '';
      started = false;
    } else {
      token += char;
      started = true;
    }
  }
  if (quote) throw new Error('Close the quoted argument before applying.');
  if (started) result.push(token);
  if (result.length > 128) throw new Error('Use at most 64 encoder overrides.');
  return result;
}

export function parseEncoderCommand(
  text: string,
  catalog: EncoderParameterCatalog,
): EncoderParameter[] {
  const tokens = commandTokens(text);
  const parameters: EncoderParameter[] = [];
  for (let i = 0; i < tokens.length; i++) {
    const equal = tokens[i].indexOf('=');
    const name = equal < 0 ? tokens[i] : tokens[i].slice(0, equal);
    const negativeSwitch =
      catalog.encoder === 'x265Standalone' && ['--no-sao', '--no-cutree'].includes(name);
    const lookup = negativeSwitch ? name.replace('--no-', '--') : name;
    const spec = catalog.parameters.find(
      (item) => item.argument === lookup || item.name === lookup,
    );
    if (!spec)
      throw new Error(
        `“${name}” is not an editable option for this encoder. Use the source, output, quality and processing controls for those settings.`,
      );
    const booleanSwitch =
      catalog.encoder === 'x265Standalone' && ['sao', 'cutree'].includes(spec.name);
    const singletonSwitch = catalog.encoder === 'aomAv1' && spec.name === 'disable-kf';
    const value =
      singletonSwitch && equal < 0
        ? '1'
        : negativeSwitch && equal < 0
          ? '0'
          : booleanSwitch &&
              equal < 0 &&
              (tokens[i + 1] === undefined || tokens[i + 1].startsWith('--'))
            ? '1'
            : equal < 0
              ? tokens[++i]
              : tokens[i].slice(equal + 1);
    if (negativeSwitch && equal >= 0) throw new Error(`Use ${name} without a value.`);
    if (value === undefined || value === '') throw new Error(`Provide a value for ${name}.`);
    if (parameters.some((item) => item.name === spec.name))
      throw new Error(`${name} appears more than once. Keep one explicit value.`);
    parameters.push({ name: spec.name, value });
  }
  const issue = parameterError(parameters, catalog, catalog.encoder);
  if (issue) throw new Error(issue);
  return parameters;
}

export function formatEncoderCommand(
  values: EncoderParameter[],
  catalog: EncoderParameterCatalog,
): string {
  return values
    .map((value) => {
      const argument =
        catalog.parameters.find((spec) => spec.name === value.name)?.argument ?? value.name;
      if (catalog.encoder === 'aomAv1' && value.name === 'disable-kf') return argument;
      if (catalog.encoder === 'x265Standalone' && ['sao', 'cutree'].includes(value.name))
        return value.value === '0' ? `--no-${value.name}` : argument;
      if (catalog.encoder === 'aomAv1' || catalog.encoder === 'vpxStandalone')
        return `${argument}=${JSON.stringify(value.value)}`;
      return `${argument} ${JSON.stringify(value.value)}`;
    })
    .join('\n');
}

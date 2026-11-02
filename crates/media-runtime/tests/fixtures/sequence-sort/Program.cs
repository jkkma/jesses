using System.Globalization;
using System.Text.Json;
var requested = args.Length > 0 ? args[0] : "CURRENT";
if (requested != "CURRENT") CultureInfo.CurrentCulture = CultureInfo.GetCultureInfo(requested);
var names = JsonSerializer.Deserialize<string[]>(Console.In.ReadToEnd())!;
// Exact source IoUtils.GetFilesSorted comparer and stable OrderBy semantics.
Console.Write(JsonSerializer.Serialize(new { locale = CultureInfo.CurrentCulture.Name, sortLocale = CultureInfo.CurrentCulture.CompareInfo.Name, ordered = names.OrderBy(x => x).ToArray() }));

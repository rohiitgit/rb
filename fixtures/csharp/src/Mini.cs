using Microsoft.Extensions.DependencyInjection;
using Shop;

// Contract M6k S3: minimal hosting. These statements have no enclosing definition,
// so the engine anchors the registration at its bound type (`Mini`, declared below);
// `Mini`'s constructor then wires like any registered consumer.
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddSingleton<Mini>();

public class Mini
{
    public Mini(Helper helper)
    {
    }
}

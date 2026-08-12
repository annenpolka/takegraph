#if TAKEGRAPH_YMM4_CONTRACT_STUB
// CI compiles the reflection-based bridge without redistributing YMM4.
// Production builds leave this symbol unset and reference YMM4's real contract.
namespace YukkuriMovieMaker.Plugin;

public interface IToolPlugin
{
    string Name { get; }

    Type ViewModelType { get; }

    Type ViewType { get; }
}
#endif

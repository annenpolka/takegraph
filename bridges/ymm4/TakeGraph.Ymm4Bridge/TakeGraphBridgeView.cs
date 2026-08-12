using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows;
using System.Windows.Controls;

namespace TakeGraph.Ymm4Bridge;

public sealed class TakeGraphBridgeViewModel : INotifyPropertyChanged
{
    public string Status => TakeGraphToolPlugin.Current.Status;
    public string CredentialPath => BridgeCredentials.CredentialsPath;

    public event PropertyChangedEventHandler? PropertyChanged;

    internal void Refresh()
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(Status)));
    }
}

public sealed class TakeGraphBridgeView : UserControl
{
    public TakeGraphBridgeView()
    {
        var status = new TextBlock { TextWrapping = TextWrapping.Wrap };
        status.SetBinding(TextBlock.TextProperty, nameof(TakeGraphBridgeViewModel.Status));
        var credential = new TextBlock { TextWrapping = TextWrapping.Wrap };
        credential.SetBinding(TextBlock.TextProperty, nameof(TakeGraphBridgeViewModel.CredentialPath));
        Content = new StackPanel
        {
            Margin = new Thickness(16),
            Children =
            {
                new TextBlock
                {
                    Text = "TakeGraph YMM4 Bridge",
                    FontSize = 18,
                    FontWeight = FontWeights.SemiBold,
                    Margin = new Thickness(0, 0, 0, 12),
                },
                status,
                new TextBlock
                {
                    Text = "Credential file",
                    FontWeight = FontWeights.SemiBold,
                    Margin = new Thickness(0, 12, 0, 4),
                },
                credential,
            },
        };
    }
}

using System.Diagnostics;

namespace TakeGraph.Ymm4Bridge;

internal sealed class RenderExecutionControl : IDisposable
{
    private readonly object gate = new();
    private readonly CancellationTokenSource cancellation = new();
    private Process? process;
    private RenderPathNamespaceLease? candidateNamespaceLease;
    private bool disposed;

    internal CancellationToken Token => cancellation.Token;

    internal bool TryAttach(Process candidate)
    {
        lock (gate)
        {
            if (disposed || cancellation.IsCancellationRequested || process is not null)
            {
                return false;
            }
            process = candidate;
            return true;
        }
    }

    internal bool TryAttachCandidateNamespaceLease(RenderPathNamespaceLease lease)
    {
        lock (gate)
        {
            if (disposed || cancellation.IsCancellationRequested || candidateNamespaceLease is not null)
            {
                return false;
            }
            candidateNamespaceLease = lease;
            return true;
        }
    }

    internal void VerifyCandidateNamespaceLease()
    {
        lock (gate)
        {
            if (disposed || candidateNamespaceLease is null)
            {
                throw new RenderSourceDriftException(
                    "The task-private render namespace lease is unavailable");
            }
            candidateNamespaceLease.Verify();
        }
    }

    internal bool TryAdvance(Action transition)
    {
        lock (gate)
        {
            if (disposed || cancellation.IsCancellationRequested)
            {
                return false;
            }
            transition();
            return true;
        }
    }

    internal bool TryComplete(Action transition)
    {
        lock (gate)
        {
            if (disposed)
            {
                return false;
            }
            transition();
            return true;
        }
    }

    internal bool TryCancel(Func<bool> persistCancellation)
    {
        Process? attached;
        lock (gate)
        {
            if (disposed)
            {
                return false;
            }
            if (!persistCancellation())
            {
                return false;
            }
            cancellation.Cancel();
            attached = process;
        }
        TryKill(attached);
        return true;
    }

    internal void Detach(Process candidate)
    {
        lock (gate)
        {
            if (ReferenceEquals(process, candidate))
            {
                process = null;
            }
        }
    }

    public void Dispose()
    {
        Process? attached;
        RenderPathNamespaceLease? namespaceLease;
        lock (gate)
        {
            if (disposed)
            {
                return;
            }
            disposed = true;
            attached = process;
            process = null;
            namespaceLease = candidateNamespaceLease;
            candidateNamespaceLease = null;
        }
        TryKill(attached);
        namespaceLease?.Dispose();
        cancellation.Dispose();
    }

    private static void TryKill(Process? candidate)
    {
        if (candidate is null)
        {
            return;
        }
        try
        {
            if (!candidate.HasExited)
            {
                candidate.Kill(entireProcessTree: true);
            }
        }
        catch (InvalidOperationException)
        {
            // The process exited between observation and cancellation.
        }
    }
}

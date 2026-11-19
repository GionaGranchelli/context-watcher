"""Create a clean detached workspace and preserve its tracked changes."""
import re
import subprocess


def git(directory, *args, remaining=None):
    return subprocess.check_output(
        ["git", "-C", str(directory), *args],
        timeout=min(30, remaining()) if remaining else 30,
    )


def checkout(repository, destination, revision, *, remaining=None):
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("Expected a full frozen Git commit SHA")
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(destination)
    git(repository, "worktree", "add", "--detach", str(destination.resolve()), revision, remaining=remaining)
    if git(destination, "rev-parse", "HEAD", remaining=remaining).decode().strip() != revision:
        raise RuntimeError("Worktree revision does not match the frozen commit")
    if git(destination, "status", "--porcelain", "--untracked-files=all", remaining=remaining):
        raise RuntimeError("Worktree is not clean")


def snapshot(workspace, *, remaining=None):
    return git(workspace, "write-tree", remaining=remaining).decode().strip()


def diff(workspace, baseline, *, remaining=None):
    # Capture the actual final worktree, including staged and untracked changes.
    git(workspace, "add", "--all", remaining=remaining)
    return git(workspace, "diff", "--cached", "--no-ext-diff", "--binary", baseline, "--",
               remaining=remaining)


def cleanup(repository, destination, *, remaining=None):
    git(repository, "worktree", "remove", "--force", str(destination.resolve()), remaining=remaining)

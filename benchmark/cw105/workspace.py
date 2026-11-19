"""Create a clean detached workspace and preserve its tracked changes."""
import re
import subprocess


def git(directory, *args):
    return subprocess.check_output(
        ["git", "-C", str(directory), *args], timeout=30,
    )


def checkout(repository, destination, revision):
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("Expected a full frozen Git commit SHA")
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(destination)
    git(repository, "worktree", "add", "--detach", str(destination.resolve()), revision)
    if git(destination, "rev-parse", "HEAD").decode().strip() != revision:
        raise RuntimeError("Worktree revision does not match the frozen commit")
    if git(destination, "status", "--porcelain", "--untracked-files=all"):
        raise RuntimeError("Worktree is not clean")


def snapshot(workspace):
    return git(workspace, "write-tree").decode().strip()


def diff(workspace, baseline):
    # Capture the actual final worktree, including staged and untracked changes.
    git(workspace, "add", "--all")
    return git(workspace, "diff", "--cached", "--no-ext-diff", "--binary", baseline, "--")


def cleanup(repository, destination):
    git(repository, "worktree", "remove", "--force", str(destination.resolve()))

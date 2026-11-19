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


def diff(workspace, revision):
    # Intent-to-add puts new files in the diff without changing their contents.
    git(workspace, "add", "--intent-to-add", "--all")
    return git(workspace, "diff", "--no-ext-diff", "--binary", "--")


def cleanup(repository, destination):
    git(repository, "worktree", "remove", "--force", str(destination.resolve()))

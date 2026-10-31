"""A private container for building a directory, and removing only what was built in it.

`worker` clones into a container this process creates: `mkdtemp` makes a directory with an
unpredictable name and mode 0700 in the destination's parent, and everything is built inside it (the
clone goes to a fresh child path, because Pijul refuses an existing directory). The finished
directory is promoted to its public name by an exclusive atomic rename between two open directories,
so the public name is never created, opened or removed any other way. Afterwards, and on any
failure, only the container and its contents are removed, through descriptors opened on it.

Threat model: this protects against concurrent `ck` commands, Pijul and a user acting on visible
paths. A same-user process that deliberately races on the private, randomly named container is out
of scope (it already holds the user's privileges); the guarantee is not claimed against it.
"""

import contextlib
import os
import stat
import tempfile
from pathlib import Path

from . import lease


def remove_contents(directory_fd: int, *, missing_ok: bool = False) -> None:
    """Remove everything inside the directory open as `directory_fd`, never following a symlink.

    With `missing_ok` an entry that another cleanup already removed is not an error."""
    for name in os.listdir(directory_fd):
        try:
            info = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
            if stat.S_ISDIR(info.st_mode):
                child = os.open(
                    name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory_fd
                )
                try:
                    opened = os.fstat(child)
                    if (opened.st_dev, opened.st_ino) == (info.st_dev, info.st_ino):
                        remove_contents(child, missing_ok=missing_ok)  # still the listed directory
                finally:
                    os.close(child)
                os.rmdir(name, dir_fd=directory_fd)
            else:
                os.unlink(name, dir_fd=directory_fd)
        except FileNotFoundError:
            if not missing_ok:
                raise


def remove_tree(parent_fd: int, name: str, *, missing_ok: bool = False) -> None:
    """Remove the directory `name` inside the open `parent_fd`, contents first, through descriptors.

    The name is opened without following a symlink and its inode is checked; only an empty directory
    is ever removed by name (`rmdir`), so a replacement that someone filled is never deleted."""
    try:
        directory = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent_fd)
    except FileNotFoundError:
        if missing_ok:
            return
        raise
    try:
        remove_contents(directory, missing_ok=missing_ok)
    finally:
        os.close(directory)
    try:
        os.rmdir(name, dir_fd=parent_fd)
    except FileNotFoundError:
        if not missing_ok:
            raise


class Container:
    """A private directory this process created in `parent`, held open from the moment it exists."""

    def __init__(self, parent: Path | str, prefix: str) -> None:
        self.parent = Path(parent)
        self.parent_fd = os.open(self.parent, os.O_RDONLY | os.O_DIRECTORY)
        self.fd = -1
        try:
            self.path = Path(tempfile.mkdtemp(prefix=prefix, dir=self.parent))
            self.name = self.path.name
            self.fd = os.open(
                self.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=self.parent_fd
            )
            opened = os.fstat(self.fd)
            self.identity = (opened.st_dev, opened.st_ino)
        except BaseException:
            self.close()
            raise

    def promote(self, name: str, destination: str) -> None:
        """Move `name` out of the container to `destination` (a name in the container's parent), only
        if nothing exists there. FileExistsError if the destination is taken."""
        lease.rename_exclusive_at(self.fd, name, self.parent_fd, destination)

    def discard(self) -> None:
        """Remove the container's contents through its descriptor, then the container itself."""
        if self.fd < 0:
            return
        remove_contents(self.fd)
        try:
            named = os.stat(self.name, dir_fd=self.parent_fd, follow_symlinks=False)
        except OSError:
            return
        if (named.st_dev, named.st_ino) == self.identity:
            with contextlib.suppress(OSError):
                os.rmdir(self.name, dir_fd=self.parent_fd)

    def close(self) -> None:
        for descriptor in (self.fd, self.parent_fd):
            if descriptor >= 0:
                with contextlib.suppress(OSError):
                    os.close(descriptor)
        self.fd = self.parent_fd = -1

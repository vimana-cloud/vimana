#!/usr/bin/env bash

set -e

# Assume that the first block device of type "disk" is the target for partitioning.
< <(lsblk --noheadings --filter='TYPE == "disk"' --output=NAME) read disk
disk="/dev/$disk"

# Device names with trailing digits tend to use `p<i>` as the partition suffix.
# Other device names tend to use a simple numerical suffix.
if [[ "$disk" =~ [0-9]$ ]]; then
  boot_partition="${disk}p1"
  root_partition="${disk}p2"
else
  boot_partition="${disk}1"
  root_partition="${disk}2"
fi

# GPT (UEFI) is backwards-compatible with MBR (BIOS)
parted "$disk" --script mklabel gpt

# The start and end addresses of the boot partition.
# The first 1MiB are used for the partition table,
# so the boot partition is 255MiB.
boot_partition_start='1MiB'
boot_partition_end='256MiB'
if [ -d /sys/firmware/efi ]
then
  parted "$disk" --script --align=optimal \
    mkpart ESP fat32 "$boot_partition_start" "$boot_partition_end"
  mkfs.fat -F 32 -n 'boot' "$boot_partition"
else
  parted "$disk" --script --align=optimal \
    mkpart primary ext4 "$boot_partition_start" "$boot_partition_end"
  mkfs.ext4 -L 'boot' "$boot_partition"
fi
parted "$disk" --script set 1 boot on

# Fill the rest of the disk with the root partition.
parted "$disk" --script --align=optimal mkpart primary ext4 "$boot_partition_end" 100%
mkfs.ext4 -L 'root' "$root_partition"

# TODO: Set up encryption for the root partition.

# Reflector populates the pacman mirror list with the latest and fastest mirrors.
echo 'Waiting for reflector to finish...'
until [[ "$(systemctl is-active reflector)" == "inactive" ]]
do
    sleep 1s
done

# Mount the new partitions under `/mnt`.
mount "$root_partition" /mnt
mount --mkdir "$boot_partition" /mnt/boot

# Set up all the packages a Vimana node needs.
pacstrap -K /mnt base linux linux-firmware grub containerd kubernetes-node kubernetes-tools openssh

# Copy over the static assets.
cp /workd /mnt/usr/bin/workd
cp /workd.service /mnt/etc/systemd/system/workd.service
mkdir -p /mnt/opt/cni/bin
cp /host-local /mnt/opt/cni/bin/host-local
chmod +x /mnt/usr/bin/workd /mnt/opt/cni/bin/host-local

# Use the same mirror list we already generated with reflector in the VM.
cp /etc/pacman.d/mirrorlist /mnt/etc/pacman.d/mirrorlist

# Generate the filesystem table based on partition UUID.
genfstab -U /mnt >> /mnt/etc/fstab

# TODO: SSH should only be possible via private keys, not a password.
arch-chroot /mnt <<-CHROOT
  echo 'root:root' | chpasswd
  pacman --sync --refresh --refresh --noconfirm
  systemctl enable containerd sshd
  echo 'Successfully provisioned Vimana node'
CHROOT

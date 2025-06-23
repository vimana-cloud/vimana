packer {
  required_plugins {
    virtualbox = {
      version = "~> 1"
      source = "github.com/hashicorp/virtualbox"
    }
  }
}

# Path to the destination directory for the resulting VMDK and OVF files.
variable "output_directory" {
  type = string
}

# Debian release version: https://cdimage.debian.org/debian-cd/
variable "debian_version" {
  type = string
}

variable "kubernetes_version" {
  type = string
}

# Path to the provisioning script to run over SSH.
variable "preseed_template" {
  type = string
}

# Path to the compiled `workd` binary to preload on the VM.
variable "workd_binary" {
  type = string
}

# Path to the `workd` SystemD service file to preload and enable on the VM.
variable "workd_service" {
  type = string
}

# Path to the `host-local` IPAM plugin binary to preload on the VM.
variable "host_local_ipam" {
  type = string
}

source "virtualbox-iso" "debian-64" {
  output_directory = "${var.output_directory}"
  # The guest OS type being installed.
  # By default this is `other`,
  # but you can get dramatic performance improvements by setting this to the proper value.
  # To view all available values for this run `VBoxManage list ostypes`.
  # Setting the correct value hints to VirtualBox how to optimize the virtual hardware
  # to work best with that operating system.
  guest_os_type = "Debian_64"
  # Download installation media.
  iso_urls = [
    "https://cdimage.debian.org/debian-cd/${var.debian_version}/amd64/iso-cd/debian-${var.debian_version}-amd64-netinst.iso",
    "https://mirrors.ocf.berkeley.edu/debian-cd/${var.debian_version}/amd64/iso-cd/debian-${var.debian_version}-amd64-netinst.iso",
  ]
  # TODO: Verify checksum signatures.
  iso_checksum = "file:https://cdimage.debian.org/debian-cd/${var.debian_version}/amd64/iso-cd/SHA256SUMS"
  # Don't bother showing the VirtualBox GUI.
  # Most of the provisioning occurs via SSH anyway.
  #headless = true
  # The time to wait after booting the initial virtual machine
  # before typing the `boot_command`.
  boot_wait = "10s"
  # An array of commands to type when the virtual machine is first booted.
  # The goal of these commands should be to type just enough
  # to initialize the operating system installer.
  # Arch Linux is pretty much ready to go.
  # We just have to set a password for `root` so the SSH provisioner can connect.
  boot_command = [
    # Exit the TUI installer.
    "<wait10s><esc><wait1s>",
    # Set the password for `root` to `root`.
    # These are the SSH credentials for the installation media, not the resulting VM.
    #"passwd<enter><wait1s>",
    # Type the password.
    #"root<enter><wait1s>",
    # Re-type the same password.
    #"root<enter><wait1s>",
    "install",
    # Delay the locale and keyboard questions
    # until after there has been a chance to preseed them.
    " auto-install/enable=true",
    # The installer asks for a hostname and domain before downloading the preseed file,
    # so these have to be set on the command line.
    " netcfg/get_hostname=unassigned-hostname",
    " netcfg/get_domain=unassigned-domain",
    " preseed/url=http://{{ .HTTPIP }}:{{ .HTTPPort }}/preseed.cfg<wait10s><enter>",
  ]
  http_content = {
    "/preseed.cfg" = templatefile("${var.preseed_template}", { kubernetes_version = "${var.kubernetes_version}" })
  }
  ssh_username = "root"
  ssh_password = "root"
  # The command to use to gracefully shut down the machine once all the provisioning is done.
  shutdown_command = "shutdown now"
  # The following options configure the virtual machine used during the build process:
  cpus = 4
  memory = 4096
}

build {
  sources = ["source.virtualbox-iso.debian-64"]

  # Files are uploaded to the installation media.
  # The provisioner script still has to copy them to the hard disk.

  provisioner "file" {
    source = "${var.workd_binary}"
    destination = "/workd"
  }

  provisioner "file" {
    source = "${var.workd_service}"
    destination = "/workd.service"
  }

  provisioner "file" {
    source = "${var.host_local_ipam}"
    destination = "/host-local"
  }

  #provisioner "shell" {
  #  script = "${var.provision_script}"
  #}
}

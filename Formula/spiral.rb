class Spiral < Formula
  desc "Spiral: PostgreSQL extension for advanced analytics"
  homepage "https://github.com/spiral-database/spiral" # Update with actual homepage
  url "https://github.com/jonatas/spiral/archive/refs/tags/v0.0.1.tar.gz" # Update with actual tag
  sha256 "0019dfc4b32d63c1392aa264aed2253c1e0c2fb09216f8e2cc269bbfb8bb49b5"
  license "Apache-2.0" # Update with actual license

  depends_on "rust" => :build
  depends_on "pkg-config" => :build
  depends_on "postgresql@18"

  def install
    # Ensure cargo-pgrx is available
    system "cargo", "install", "cargo-pgrx", "--version", "0.17.0", "--locked"
    
    # Initialize pgrx pointing to the brew-installed PostgreSQL 18
    pg_config = Formula["postgresql@18"].opt_bin/"pg_config"
    system "cargo", "pgrx", "init", "--pg18=#{pg_config}"
    
    # Build the package
    system "cargo", "pgrx", "package", "--features", "pg18"
    
    # Files are generated in target/release/spiral-pg18/
    cd "target/release/spiral-pg18" do
      # Install into the formula's prefix
      prefix.install Dir["*"]
    end
  end

  def caveats
    <<~EOS
      The spiral extension has been installed to:
        #{opt_prefix}

      To finish the installation, you must link the extension files into the PostgreSQL 18 directory.
      You can do this by running:
        
        mkdir -p $(pg_config --pkglibdir)
        mkdir -p $(pg_config --sharedir)/extension
        
        ln -sf #{opt_prefix}/spiral.dylib $(pg_config --pkglibdir)/spiral.so
        ln -sf #{opt_prefix}/spiral.control $(pg_config --sharedir)/extension/
        ln -sf #{opt_prefix}/spiral--*.sql $(pg_config --sharedir)/extension/

      After linking, you can enable the extension in PostgreSQL with:
        CREATE EXTENSION spiral;
    EOS
  end

  test do
    # Simple test to check if pg_config is available and the prefix has files
    assert_predicate prefix/"spiral.control", :exist?
  end
end

use std::{io::{Read, Seek, BufReader, Write, BufWriter, SeekFrom, self}, path::{PathBuf, Path}, collections::HashMap, fs::{File, self, OpenOptions}, ffi::OsStr};

use serde::{Deserialize, Serialize};
use serde_json::Deserializer;

use crate::{error::{Result, KvsError}};

const COMPACTION_THRESHOLD:u64 = 1024 * 1024;

pub struct KvStore {
    path: PathBuf,
    readers: HashMap<u64, BufReaderWithPos<File>>,
    writer: BufWriterWithPos<File>,
    index: HashMap<String, CommandPos>,
    current_gen: u64,

    // 压缩后可以保存的字节数。
    uncompacted:u64,
}

impl KvStore {
    // 通过文件夹路径开启一个KvStore
    pub fn open(path: impl Into<PathBuf>) -> Result<KvStore> {

        let path = path.into();

        fs::create_dir_all(&path)?;

        let mut readers = HashMap::<u64, BufReaderWithPos<File>>::new();

        let mut index= HashMap::<String,CommandPos>::new();

        let gen_list = sorted_gen_list(&path)?;

        let mut uncompacted = 0;

        // 对读入其Map进行初始化并计算对应的压缩阈值
        for &gen in &gen_list {
            let mut reader = BufReaderWithPos::new(File::open(log_path(&path, gen))?)?;
            uncompacted += load(gen, &mut reader, &mut index)?;
            readers.insert(gen, reader);
        }

        // 获取当前最新的写入序名（之前的+1）
        let current_gen = gen_list.last().unwrap_or(&0) + 1;

        // 以最新的写入序名创建新的日志文件
        let writer = new_log_file(&path, current_gen, &mut readers)?;
        Ok(KvStore{
            path,
            readers,
            writer,
            index,
            current_gen,
            uncompacted
        })
    }

    /// 存入数据
    pub fn set(&mut self,key:String,value:String) -> Result<()> {

        let cmd = Command::set(key, value);

        // 获取写入器当前地址
        let pos = self.writer.pos;

        // 以json形式写入该命令
        serde_json::to_writer(&mut self.writer, &cmd)?;

        // 刷入文件中
        self.writer.flush()?;

        // 当模式匹配cmd为正确时
        if let Command::Set {key,..} = cmd{
            // 封装为CommandPos
            let cmd_pos = CommandPos{
                gen:self.current_gen,
                pos,
                len:self.writer.pos - pos
            };

            // 将封装ComandPos存入索引Map中
            if let Some(old_cmd) = self.index.insert(key,cmd_pos) {
                // 将阈值提升至该命令的大小
                self.uncompacted += old_cmd.len;
            }
            if self.uncompacted > COMPACTION_THRESHOLD {
                // 当阈值达到时
            }
        }

        // 阈值过高进行压缩
        if self.uncompacted > COMPACTION_THRESHOLD {
            self.compact()?
        }

        // 获取写入器当前地址
        Ok(())
    }

    /// 获取数据
    pub fn get(&mut self, key:String) -> Result<Option<String>> {

        // 若index中获取到了该数据命令
        if let Some(cmd_pos) = self.index.get(&key) {
            // 从读取器Map中通过该命令的序号获取对应的日志读取器
            let reader = self.readers.get_mut(&cmd_pos.gen)
                .expect(format!("Can't find reader: {}", &cmd_pos.gen).as_str());

            // 将读取器的指针切换到命令的位置中
            reader.seek(SeekFrom::Start(cmd_pos.pos))?;
            // 获取这段内容
            let cmd_reader = reader.take(cmd_pos.len);

            // 将命令进行转换
            if let Command::Set {value,  ..} = serde_json::from_reader(cmd_reader)? {
                //返回匹配成功的数据
                Ok(Some(value))
            } else {
                //返回错误（错误的指令类型）
                Err(KvsError::UnexpectedCommandType)
            }
        } else {
            Ok(None)
        }

    }

    /// 删除数据
    pub fn remove(&mut self, key:String) -> Result<()> {
        // 若index中存在这个key
        if self.index.contains_key(&key) {
            // 对这个key做命令封装
            let cmd = Command::remove(key);
            // 将这条命令以json形式写入至当前日志文件
            serde_json::to_writer(&mut self.writer, &cmd)?;
            // 刷入文件中
            self.writer.flush()?;
            // 若cmd模式匹配成功则删除该数据
            if let Command::Remove {key} = cmd{
                self.index.remove(&key).expect("key not found");
            }
            Ok(())
        } else {
            Err(KvsError::KeyNotFound)
        }
    }

    pub fn compact(&mut self) -> Result<()> {

        // 预压缩的数据位置为原文件位置的向上一位
        let compaction_gen = self.current_gen + 1;
        // 新的写入位置为原位置的向上两位
        self.current_gen += 2;
        // 写入器的位置重定向为新的写入位置
        self.writer = self.new_log_file(self.current_gen)?;

        // 初始化新的写入地址
        let mut new_pos:u64 = 0;
        // 开启压缩文件的写入器
        let mut compaction_writer = self.new_log_file(compaction_gen)?;
        // 遍历内存中保存的所有数据
        for cmd_pos in &mut self.index.values_mut() {
            // 通过该单条命令获取对应的文件读取器
            let reader = self.readers.get_mut(&cmd_pos.gen)
                .expect(format!("Can't find reader: {}",&cmd_pos.gen).as_str());

            // 如果当前读取器的地址与指令地址不一致
            if reader.pos != cmd_pos.pos {
                // 定位至命令地址
                reader.seek(SeekFrom::Start(cmd_pos.pos))?;
            }

            // 获取该命令
            let mut cmd_reader = reader.take(cmd_pos.len);
            // 将该命令拷贝到压缩文件中
            let len = io::copy(&mut cmd_reader, &mut compaction_writer)?;
            // 该命令解引用并更新为压缩文件中写入的命令
            *cmd_pos = CommandPos{
                gen:compaction_gen,
                pos:new_pos,
                len
            };
            // 写入地址累加
            new_pos += len;
        }
        // 将所有写入刷入压缩文件中
        compaction_writer.flush()?;

        // 遍历过滤出小于压缩文件序号的文件号名收集为过期Vec
        let stale_gens: Vec<_> = self.readers.keys()
            .filter(|&&gen| gen < compaction_gen)
            .cloned().collect();

        // 遍历过期Vec对数据进行旧文件删除
        for stale_gen in stale_gens {
            self.readers.remove(&stale_gen);
            fs::remove_file(log_path(&self.path, stale_gen))?;
        }
        // 将压缩阈值调整为0
        self.uncompacted = 0;

        Ok(())
    }

    // 新建日志文件方法参数封装
    fn new_log_file(&mut self, gen:u64) -> Result<BufWriterWithPos<File>> {
        new_log_file(&self.path, gen, &mut self.readers)
    }

}

#[derive(Debug)]
struct CommandPos {
    gen:u64,
    pos:u64,
    len:u64,
}


struct BufReaderWithPos<R:Read + Seek> {
    reader:BufReader<R>,
    pos:u64,
}


impl <R: Read + Seek> BufReaderWithPos<R> {
    fn new(mut inner: R) -> Result<Self> {
        let pos = inner.seek(SeekFrom::Current(0))?;
        Ok(BufReaderWithPos {
            reader: BufReader::new(inner),
            pos
        })
    }
}

impl <R: Read + Seek> Read for BufReaderWithPos<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let len = self.reader.read(buf)?;
        self.pos += len as u64;
        Ok(len)
    }
}

impl <R: Read + Seek> Seek for BufReaderWithPos<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.pos = self.reader.seek(pos)?;
        Ok(self.pos)
    }
}

struct BufWriterWithPos<W: Write + Seek> {
    writer: BufWriter<W>,
    pos: u64,
}

impl <W: Write + Seek> BufWriterWithPos<W> {
    fn new (mut inner:W) -> Result<Self> {
        let pos = inner.seek(SeekFrom::End(0))?;
        Ok({
            BufWriterWithPos {
                writer:BufWriter::new(inner),
                pos
            }
        })
    }
}

impl <W: Write + Seek> Write for BufWriterWithPos<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let len = self.writer.write(buf)?;
        self.pos += len as u64;
        Ok(len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

impl<W: Write + Seek> Seek for BufWriterWithPos<W> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.pos = self.writer.seek(pos)?;
        Ok(self.pos)
    }
}

#[derive(Serialize,Deserialize,Debug)]
pub enum Command{
    Set{
        key:String,
        value:String
    },
    Remove{
        key:String
    }
}


impl Command {
    fn set(key:String, value:String) -> Command {
        Command::Set {key,value}
    }

    fn remove(key:String) -> Command {
        Command::Remove {key}
    }
}

/// 对文件夹路径填充日志文件名
fn log_path(dir: &Path, gen :u64) -> PathBuf {
    dir.join(format!("{}.log", gen))
}

/// 通过目录地址加载数据
fn load(gen:u64, reader:&mut BufReaderWithPos<File>, index: &mut HashMap<String,CommandPos>) -> Result<u64> {
    // 将读入器地址初始化0
    let mut pos = reader.seek(SeekFrom::Start(0))?;
    // 流式读取将数据序列化为Command
    let mut stream = Deserializer::from_reader(reader).into_iter::<Command>();
    // 初始化空间占用为0
    let mut  uncompacted = 0;

    while let Some(cmd) = stream.next() {
        // 计算这段byte所在位置
        let new_pos = stream.byte_offset() as u64;
        match cmd? {

            Command::Set {key, ..} => {
                //数据插入索引之中，成功则对空间占用值进行累加
                if let Some(old_name) = index.insert(key,CommandPos{gen,pos,len:new_pos-pos}) {
                    uncompacted += old_name.len;
                }
            }

            Command::Remove {key} => {
                //索引删除该数据之中，成功则对空间占用值进行累加
                if let Some(old_cmd) = index.remove(&key) {
                    uncompacted += old_cmd.len;
                }
                uncompacted += new_pos - pos;
            }
        }
        // 写入地址等于new_pos
        pos = new_pos;
    }

    Ok(uncompacted)
}

/// 现有日志文件序号排序
fn sorted_gen_list(path: &Path) -> Result<Vec<u64>> {
    // 读取文件夹路径
    // 获取该文件夹内各个文件的地址
    // 判断是否为文件并判断拓展名是否为log
    //  对文件名进行字符串转换
    //  去除.log后缀
    //  将文件名转换为u64
    // 对数组进行拷贝并收集
    let mut gen_list:Vec<u64> = fs::read_dir(path)?
        .flat_map(|res| -> Result<_> {Ok(res?.path())})
        .filter(|path| path.is_file() && path.extension() == Some("log".as_ref()))
        .flat_map(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .map(|s| s.trim_end_matches(".log"))
                .map(str::parse::<u64>)
        }).flatten().collect();

    // 对序号进行排序
    gen_list.sort_unstable();
    // 返回排序好的Vec
    Ok(gen_list)
}


/// 新建日志文件
/// 传入文件夹路径、日志名序号、读取器Map
/// 返回对应的写入器
fn new_log_file(path:&Path, gen:u64, readers:&mut HashMap<u64, BufReaderWithPos<File>>) -> Result<BufWriterWithPos<File>> {
    // 得到对应日志的路径
    let path = log_path(path, gen);

    // 通过路径构造写入器
    let writer = BufWriterWithPos::new(OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open(&path)?)?;

    // 构造读取器并填充到读取器Map中
    readers.insert(gen, BufReaderWithPos::new(File::open(&path)?)?);
    //返回该写入器
    Ok(writer)
}
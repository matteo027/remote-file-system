import { Entity, Column, PrimaryColumn, ManyToOne, JoinColumn } from "typeorm";
import { User } from "./User";

@Entity()
export class File {
  @PrimaryColumn()
  path: string;

  @ManyToOne(() => User, (user) => user.files)
  @JoinColumn({ name: "owner" })
  owner: User;

  @Column()
  type: number; // 0 = file, 1 = directory, 2 = symlink, etc.

  @Column()
  permissions: string;

  @Column()
  group: string;

  @Column()
  size: number;

  @Column()
  atime: number; // last access time

  @Column()
  mtime: number; // last modification time

  @Column()
  ctime: number; // last time that file's metadata (e.g., permissions) was last changed

  @Column()
  btime: number; // birth time
}